//! Ordered local typing with one authoritative command in flight. Publications
//! alone never confirm an edit; both its result and exact source text are needed.
use crate::interaction::{editing_context, CommandBinding, EditingContext};
use rui_protocol_core::{
    model::*,
    resources::{MemoryBudget, MemoryReservation},
};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TypingError {
    #[error("editing context changed; pending typing cancelled")]
    Context,
    #[error("typing queue or text exceeds the local limit")]
    Limit,
    #[error("typing allocation limit reached")]
    Memory,
    #[error("text command was rejected or has no causal source evidence")]
    Result,
}
struct Input {
    text: String,
    _memory: MemoryReservation,
}
struct Expected {
    sequence: u64,
    before: String,
    after: String,
    selection: TextRange,
    result: Option<ConfirmedResult>,
    _memory: MemoryReservation,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct ConfirmedResult {
    source_commit: [u8; 32],
    revision: Option<u64>,
}
pub struct PreparedText {
    pub context: EditingContext,
    pub text: String,
    pub memory: MemoryReservation,
}
pub struct TypingQueue {
    budget: MemoryBudget,
    inputs: VecDeque<Input>,
    bytes: usize,
    origin: Option<EditingContext>,
    confirmed: Option<EditingContext>,
    pending: Option<Expected>,
    _memory: MemoryReservation,
}
impl TypingQueue {
    pub fn new(budget: MemoryBudget) -> Result<Self, TypingError> {
        let memory = budget
            .reserve(32 * std::mem::size_of::<Input>() * 2 + 2048)
            .map_err(|_| TypingError::Memory)?;
        let mut inputs = VecDeque::new();
        inputs
            .try_reserve_exact(32)
            .map_err(|_| TypingError::Memory)?;
        Ok(Self {
            budget,
            inputs,
            bytes: 0,
            origin: None,
            confirmed: None,
            pending: None,
            _memory: memory,
        })
    }
    pub fn context(&self) -> Option<EditingContext> {
        self.origin
    }
    pub fn pending_count(&self) -> usize {
        self.inputs.len() + usize::from(self.pending.is_some())
    }
    pub fn cancel(&mut self) -> usize {
        let count = self.pending_count();
        self.inputs.clear();
        self.bytes = 0;
        self.pending = None;
        self.origin = None;
        self.confirmed = None;
        count
    }
    pub fn accept(
        &mut self,
        captured: EditingContext,
        text: String,
        mut memory: MemoryReservation,
        state: &State,
        binding: &CommandBinding,
    ) -> Result<(), TypingError> {
        if text.len() > 1024
            || self.pending_count() >= 32
            || self
                .bytes
                .checked_add(text.len())
                .is_none_or(|n| n > 16 * 1024)
        {
            return Err(TypingError::Limit);
        }
        memory
            .resize(
                text.capacity()
                    .checked_add(2048)
                    .ok_or(TypingError::Limit)?,
            )
            .map_err(|_| TypingError::Memory)?;
        if let Some(origin) = self.origin {
            if origin != captured {
                return Err(TypingError::Context);
            }
        } else {
            if editing_context(state, captured.target, binding) != Some(captured) {
                return Err(TypingError::Context);
            }
            self.origin = Some(captured);
            self.confirmed = Some(captured);
        }
        self.bytes += text.len();
        self.inputs.push_back(Input {
            text,
            _memory: memory,
        });
        Ok(())
    }
    pub fn start(
        &mut self,
        sequence: u64,
        state: &State,
        binding: &CommandBinding,
    ) -> Result<Option<PreparedText>, TypingError> {
        if self.pending.is_some() || self.inputs.is_empty() {
            return Ok(None);
        }
        let context = self.confirmed.ok_or(TypingError::Context)?;
        if editing_context(state, context.target, binding) != Some(context) {
            return Err(TypingError::Context);
        }
        let Record::Text(model) = state
            .records
            .get(&context.text)
            .ok_or(TypingError::Context)?
        else {
            return Err(TypingError::Context);
        };
        let before = model.content.as_deref().ok_or(TypingError::Context)?;
        let input = self.inputs.front().ok_or(TypingError::Context)?;
        let lower = context.selection.anchor.min(context.selection.focus);
        let upper = context.selection.anchor.max(context.selection.focus);
        let index = |scalar: u64| {
            if scalar == before.chars().count() as u64 {
                Some(before.len())
            } else {
                usize::try_from(scalar)
                    .ok()
                    .and_then(|i| before.char_indices().nth(i).map(|(byte, _)| byte))
            }
        };
        let start = index(lower).ok_or(TypingError::Context)?;
        let end = index(upper).ok_or(TypingError::Context)?;
        let length = start
            .checked_add(input.text.len())
            .and_then(|n| n.checked_add(before.len() - end))
            .ok_or(TypingError::Limit)?;
        if length > 64 * 1024 || before.len() > 64 * 1024 {
            return Err(TypingError::Limit);
        }
        let memory = self
            .budget
            .reserve(before.len() + length + 2048)
            .map_err(|_| TypingError::Memory)?;
        let mut old = String::new();
        old.try_reserve_exact(before.len())
            .map_err(|_| TypingError::Memory)?;
        old.push_str(before);
        let mut after = String::new();
        after
            .try_reserve_exact(length)
            .map_err(|_| TypingError::Memory)?;
        after.push_str(&before[..start]);
        after.push_str(&input.text);
        after.push_str(&before[end..]);
        let cursor = lower
            .checked_add(input.text.chars().count() as u64)
            .ok_or(TypingError::Limit)?;
        let input = self.inputs.pop_front().ok_or(TypingError::Context)?;
        self.bytes -= input.text.len();
        self.pending = Some(Expected {
            sequence,
            before: old,
            after,
            selection: TextRange {
                anchor: cursor,
                focus: cursor,
            },
            result: None,
            _memory: memory,
        });
        Ok(Some(PreparedText {
            context,
            text: input.text,
            memory: input._memory,
        }))
    }
    /// Only the matching authenticated terminal result with actual source
    /// evidence can confirm an edit. A coalesced publication may not yet have
    /// an installed view revision; its exact text/context still must be observed.
    /// Dispatched is nonterminal. Other sequences cannot advance this FIFO.
    pub fn result(
        &mut self,
        sequence: u64,
        status: &str,
        source_commit: &str,
        revision: Option<u64>,
    ) -> Result<(), TypingError> {
        let Some(pending) = &mut self.pending else {
            return Ok(());
        };
        if pending.sequence != sequence {
            return Ok(());
        }
        if status == "Dispatched" || status == "DISPATCHED" {
            return Ok(());
        }
        let previous = self.confirmed.ok_or(TypingError::Context)?;
        if (status != "Applied" && status != "APPLIED")
            || source_commit.is_empty()
            || source_commit.len() > 1024
            || revision.is_some_and(|v| v <= previous.revision)
        {
            return Err(TypingError::Result);
        }
        let result = ConfirmedResult {
            source_commit: Sha256::digest(source_commit.as_bytes()).into(),
            revision,
        };
        if pending.result.is_some_and(|old| old != result) {
            return Err(TypingError::Result);
        }
        pending.result = Some(result);
        Ok(())
    }
    /// Returns true only when an in-flight edit is causally confirmed. A source
    /// context change without the exact expected replacement fails closed.
    pub fn observe(
        &mut self,
        state: &State,
        binding: &CommandBinding,
    ) -> Result<bool, TypingError> {
        let Some(previous) = self.confirmed else {
            return Ok(false);
        };
        let actual =
            editing_context(state, previous.target, binding).ok_or(TypingError::Context)?;
        let Some(pending) = &self.pending else {
            return if actual == previous {
                Ok(false)
            } else {
                Err(TypingError::Context)
            };
        };
        let Record::Text(model) = state
            .records
            .get(&actual.text)
            .ok_or(TypingError::Context)?
        else {
            return Err(TypingError::Context);
        };
        let content = model.content.as_deref().ok_or(TypingError::Context)?;
        let same_identity = (
            actual.epoch,
            actual.view,
            actual.lease,
            actual.generation,
            actual.target,
            actual.text,
            actual.focus_generation,
        ) == (
            previous.epoch,
            previous.view,
            previous.lease,
            previous.generation,
            previous.target,
            previous.text,
            previous.focus_generation,
        );
        if !same_identity {
            return Err(TypingError::Context);
        }
        let content_changed = pending.before != pending.after;
        let target_clock = actual.target_version == previous.target_version
            || Some(actual.target_version) == previous.target_version.checked_add(1);
        let record_clock = if content_changed || pending.selection != previous.selection {
            Some(actual.text_record_version) == previous.text_record_version.checked_add(1)
        } else {
            actual.text_record_version == previous.text_record_version
                || Some(actual.text_record_version) == previous.text_record_version.checked_add(1)
        };
        let expected = content == pending.after
            && actual.selection == pending.selection
            && target_clock
            && record_clock
            && if content_changed {
                Some(actual.text_version) == previous.text_version.checked_add(1)
            } else {
                actual.text_version == previous.text_version
            };
        let unchanged = actual == previous && content == pending.before;
        if !expected && !unchanged {
            return Err(TypingError::Context);
        }
        if pending.result.is_some_and(|result| {
            state.revision > previous.revision
                && result
                    .revision
                    .is_none_or(|revision| state.revision >= revision)
                && (result.revision.is_some() || expected)
        }) {
            if !expected {
                return Err(TypingError::Context);
            }
            self.confirmed = Some(actual);
            self.pending = None;
            return Ok(true);
        }
        Ok(false)
    }
}
