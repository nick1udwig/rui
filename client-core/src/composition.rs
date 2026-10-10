//! Bounded CLIENT_TEXT composition. Transient dispatch never confirms a document
//! edit; a commit needs its result, exact source context and capture-token witness.
use crate::interaction::{editing_context, CommandBinding, EditingContext};
use rui_protocol_core::{
    commands::{CommandHeader, CompositionOperation, Id, TextCompositionCommand},
    input::{CompositionMachine, EditingContext as SourceContext, ScalarRange, TextOwner},
    model::*,
    resources::{MemoryBudget, MemoryReservation},
};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use thiserror::Error;

pub const GTK_EDITABLE_CLOCK_PROFILE: &str = "text.clock.gtk-editable4224.v1";
pub const MAX_COMPOSITION_BYTES: usize = 16 * 1024;
const MAX_EVENTS: usize = 32;
const MAX_IDENTITIES: usize = 128;
const MAX_QUEUED_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ClockPolicy {
    /// One content change and one full text-record change, when applicable.
    Strict,
    /// Only select after explicit negotiation of GTK_EDITABLE_CLOCK_PROFILE.
    GtkEditable4224,
}
impl ClockPolicy {
    pub(crate) fn deltas(
        self,
        before: &str,
        after: &str,
        selection_before: TextRange,
        selection_after: TextRange,
    ) -> (u64, u64) {
        match self {
            Self::Strict => (
                u64::from(before != after),
                u64::from(before != after || selection_before != selection_after),
            ),
            Self::GtkEditable4224 => {
                let text = u64::from(!before.is_empty()) + u64::from(!after.is_empty());
                (text, text + u64::from(selection_after.focus > 0))
            }
        }
    }
}
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CompositionError {
    #[error("composition context changed; local preedit cancelled")]
    Context,
    #[error("composition queue, identity or text limit reached")]
    Limit,
    #[error("composition allocation limit reached")]
    Memory,
    #[error("composition stage or identity is stale")]
    Stage,
    #[error("composition result lacks matching causal source evidence")]
    Result,
}
/// Borrowed native input; the queue reserves memory before retaining its text.
pub enum NativeStage<'a> {
    Begin,
    Update {
        text: &'a str,
        selection: ScalarRange,
    },
    Commit(&'a str),
    Cancel,
}
struct Event {
    id: Id,
    operation: CompositionOperation,
    _memory: MemoryReservation,
}
struct Receipt {
    sequence: u64,
    commit: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct CommitResult {
    cause: [u8; 32],
    revision: Option<u64>,
}
struct Expected {
    sequence: u64,
    surface: u64,
    before: String,
    after: String,
    selection: TextRange,
    result: Option<CommitResult>,
    _memory: MemoryReservation,
}
pub struct PreparedComposition {
    pub command: TextCompositionCommand,
    pub memory: MemoryReservation,
}
pub struct CompositionQueue {
    budget: MemoryBudget,
    policy: ClockPolicy,
    events: VecDeque<Event>,
    bytes: usize,
    issued: Vec<Id>,
    native_active: Option<Id>,
    machine: CompositionMachine,
    origin: Option<EditingContext>,
    confirmed: Option<EditingContext>,
    receipt: Option<Receipt>,
    expected: Option<Expected>,
    _memory: MemoryReservation,
}
impl CompositionQueue {
    pub fn new(budget: MemoryBudget, policy: ClockPolicy) -> Result<Self, CompositionError> {
        // Each of the machine's at most 128 sparse BTree entries gets 512 bytes,
        // including minimally occupied nodes. Vectors and deque retain capacity
        // across cancellation; their complete capacity stays charged as well.
        let memory = budget
            .reserve(
                MAX_IDENTITIES * (512 + std::mem::size_of::<Id>())
                    + 2 * MAX_EVENTS * std::mem::size_of::<Event>()
                    + 8192,
            )
            .map_err(|_| CompositionError::Memory)?;
        let mut events = VecDeque::new();
        events
            .try_reserve_exact(MAX_EVENTS)
            .map_err(|_| CompositionError::Memory)?;
        let mut issued = Vec::new();
        issued
            .try_reserve_exact(MAX_IDENTITIES)
            .map_err(|_| CompositionError::Memory)?;
        Ok(Self {
            budget,
            policy,
            events,
            bytes: 0,
            issued,
            native_active: None,
            machine: CompositionMachine::new(MAX_IDENTITIES, MAX_COMPOSITION_BYTES)
                .map_err(|_| CompositionError::Limit)?,
            origin: None,
            confirmed: None,
            receipt: None,
            expected: None,
            _memory: memory,
        })
    }
    pub fn context(&self) -> Option<EditingContext> {
        self.origin
    }
    pub fn is_active(&self) -> bool {
        self.native_active.is_some()
            || self.machine.active_id().is_some()
            || self.receipt.is_some()
            || self.expected.is_some()
            || !self.events.is_empty()
    }
    pub fn pending_count(&self) -> usize {
        self.events.len() + usize::from(self.receipt.is_some() || self.expected.is_some())
    }
    /// The caller also cancels source authority through its priority lifecycle
    /// path. Sent ambiguous bytes belong to the common exact CommandQueue.
    /// Retired composition identities remain retired for this queue's lifetime.
    pub fn cancel(&mut self) -> usize {
        let count = self.pending_count();
        self.events.clear();
        self.bytes = 0;
        self.native_active = None;
        self.machine.set_context(None);
        self.origin = None;
        self.confirmed = None;
        self.receipt = None;
        self.expected = None;
        count
    }
    pub fn accept(
        &mut self,
        captured: EditingContext,
        id: Id,
        stage: NativeStage<'_>,
        state: &State,
        binding: &CommandBinding,
    ) -> Result<(), CompositionError> {
        let text = match &stage {
            NativeStage::Update { text, .. } | NativeStage::Commit(text) => *text,
            _ => "",
        };
        if text.len() > MAX_COMPOSITION_BYTES
            || text.contains('\0')
            || self.pending_count() >= MAX_EVENTS
            || self
                .bytes
                .checked_add(text.len())
                .is_none_or(|n| n > MAX_QUEUED_BYTES)
        {
            return Err(CompositionError::Limit);
        }
        if let Some(origin) = self.origin {
            if captured != origin {
                return Err(CompositionError::Context);
            }
        } else if editing_context(state, captured.target, binding) != Some(captured) {
            return Err(CompositionError::Context);
        }
        match &stage {
            NativeStage::Begin => {
                if id == [0; 16] || self.native_active.is_some() || self.issued.contains(&id) {
                    return Err(CompositionError::Stage);
                }
                if self.issued.len() == MAX_IDENTITIES {
                    return Err(CompositionError::Limit);
                }
            }
            _ if self.native_active != Some(id) => return Err(CompositionError::Stage),
            NativeStage::Update { selection, .. }
                if selection.start > selection.end
                    || selection.end > text.chars().count() as u64 =>
            {
                return Err(CompositionError::Stage)
            }
            _ => {}
        }
        let mut memory = self
            .budget
            .reserve(text.len() + 2048)
            .map_err(|_| CompositionError::Memory)?;
        let mut owned = String::new();
        owned
            .try_reserve_exact(text.len())
            .map_err(|_| CompositionError::Memory)?;
        owned.push_str(text);
        memory
            .resize(owned.capacity() + 2048)
            .map_err(|_| CompositionError::Memory)?;
        let operation = match stage {
            NativeStage::Begin => {
                self.issued.push(id);
                self.native_active = Some(id);
                CompositionOperation::Begin(ScalarRange { start: 0, end: 0 })
            }
            NativeStage::Update { selection, .. } => CompositionOperation::UpdatePreedit {
                text: owned,
                selection,
            },
            NativeStage::Commit(_) => {
                self.native_active = None;
                CompositionOperation::CommitText { text: owned }
            }
            NativeStage::Cancel => {
                self.native_active = None;
                CompositionOperation::Cancel
            }
        };
        if self.origin.is_none() {
            self.origin = Some(captured);
            self.confirmed = Some(captured);
        }
        self.bytes += text.len();
        self.events.push_back(Event {
            id,
            operation,
            _memory: memory,
        });
        Ok(())
    }
    /// All stages wait for the actual painted current revision. One stage is
    /// admitted at a time; a commit blocks the next composition until confirmed.
    pub fn start(
        &mut self,
        state: &State,
        binding: &CommandBinding,
        presented: Option<u64>,
    ) -> Result<Option<PreparedComposition>, CompositionError> {
        if binding.sequence == 0 {
            return Err(CompositionError::Stage);
        }
        if self.receipt.is_some()
            || self.expected.is_some()
            || self.events.is_empty()
            || presented != Some(state.revision)
        {
            return Ok(None);
        }
        let context = self.confirmed.ok_or(CompositionError::Context)?;
        if editing_context(state, context.target, binding) != Some(context) {
            return Err(CompositionError::Context);
        }
        let Record::Text(model) = state
            .records
            .get(&context.text)
            .ok_or(CompositionError::Context)?
        else {
            return Err(CompositionError::Context);
        };
        let before = model.content.as_deref().ok_or(CompositionError::Context)?;
        self.machine.set_context(Some(SourceContext {
            target: context.target,
            focus_generation: context.focus_generation,
            text_version: context.text_version,
            scalar_count: before.chars().count() as u64,
            owner: TextOwner::ClientText,
        }));
        let event = self.events.front().ok_or(CompositionError::Stage)?;
        let commit = matches!(&event.operation, CompositionOperation::CommitText { .. });
        match &event.operation {
            CompositionOperation::Begin(_) => {
                let range = ScalarRange {
                    start: context.selection.anchor.min(context.selection.focus),
                    end: context.selection.anchor.max(context.selection.focus),
                };
                self.machine
                    .begin(
                        event.id,
                        context.focus_generation,
                        context.text_version,
                        range,
                    )
                    .map_err(|_| CompositionError::Stage)?;
            }
            CompositionOperation::UpdatePreedit { text, selection } => self
                .machine
                .validate_update(event.id, text, *selection)
                .map_err(|_| CompositionError::Stage)?,
            CompositionOperation::CommitText { text } => {
                self.machine
                    .validate_commit(
                        event.id,
                        context.focus_generation,
                        context.text_version,
                        text,
                    )
                    .map_err(|_| CompositionError::Stage)?;
                let range = self.machine.active_range().ok_or(CompositionError::Stage)?;
                let surface = focus_surface(state, context)?;
                let index = |scalar: u64| {
                    if scalar == before.chars().count() as u64 {
                        Some(before.len())
                    } else {
                        usize::try_from(scalar)
                            .ok()
                            .and_then(|n| before.char_indices().nth(n).map(|(byte, _)| byte))
                    }
                };
                let start = index(range.start).ok_or(CompositionError::Context)?;
                let end = index(range.end).ok_or(CompositionError::Context)?;
                let length = start
                    .checked_add(text.len())
                    .and_then(|n| n.checked_add(before.len() - end))
                    .ok_or(CompositionError::Limit)?;
                if before.len() > 64 * 1024 || length > 64 * 1024 {
                    return Err(CompositionError::Limit);
                }
                let mut memory = self
                    .budget
                    .reserve(before.len() + length + 2048)
                    .map_err(|_| CompositionError::Memory)?;
                let mut old = String::new();
                old.try_reserve_exact(before.len())
                    .map_err(|_| CompositionError::Memory)?;
                old.push_str(before);
                let mut after = String::new();
                after
                    .try_reserve_exact(length)
                    .map_err(|_| CompositionError::Memory)?;
                after.push_str(&before[..start]);
                after.push_str(text);
                after.push_str(&before[end..]);
                memory
                    .resize(old.capacity() + after.capacity() + 2048)
                    .map_err(|_| CompositionError::Memory)?;
                let caret = range
                    .start
                    .checked_add(text.chars().count() as u64)
                    .ok_or(CompositionError::Limit)?;
                self.expected = Some(Expected {
                    sequence: binding.sequence,
                    surface,
                    before: old,
                    after,
                    selection: TextRange {
                        anchor: caret,
                        focus: caret,
                    },
                    result: None,
                    _memory: memory,
                });
                self.machine.cancel();
            }
            CompositionOperation::Cancel => {
                if self.machine.active_id() != Some(event.id) {
                    return Err(CompositionError::Stage);
                }
                self.machine.cancel();
            }
        }
        let mut event = self.events.pop_front().ok_or(CompositionError::Stage)?;
        self.bytes -= operation_bytes(&event.operation);
        if matches!(&event.operation, CompositionOperation::Begin(_)) {
            event.operation = CompositionOperation::Begin(ScalarRange {
                start: context.selection.anchor.min(context.selection.focus),
                end: context.selection.anchor.max(context.selection.focus),
            });
        }
        self.receipt = Some(Receipt {
            sequence: binding.sequence,
            commit,
        });
        Ok(Some(PreparedComposition {
            command: TextCompositionCommand {
                header: CommandHeader {
                    epoch: binding.epoch,
                    view_id: binding.view,
                    client_instance_id: binding.client,
                    command_seq: binding.sequence,
                    control_lease_id: binding.lease,
                    control_generation: binding.generation,
                    observed_view_revision: state.revision,
                },
                target_id: context.target,
                text_session_id: event.id,
                focus_generation: context.focus_generation,
                base_text_version: context.text_version,
                operation: event.operation,
            },
            memory: event._memory,
        }))
    }
    pub fn result(
        &mut self,
        sequence: u64,
        status: &str,
        cause: &str,
        revision: Option<u64>,
    ) -> Result<bool, CompositionError> {
        let commit = if let Some(receipt) = self.receipt.as_ref().filter(|v| v.sequence == sequence)
        {
            receipt.commit
        } else if self
            .expected
            .as_ref()
            .is_some_and(|v| v.sequence == sequence)
        {
            true
        } else {
            return Ok(false);
        };
        if !commit {
            if status != "Dispatched" && status != "DISPATCHED"
                || !cause.is_empty()
                || revision.is_some()
            {
                return Err(CompositionError::Result);
            }
            self.receipt = None;
            return Ok(true);
        }
        if status == "Dispatched" || status == "DISPATCHED" {
            return Ok(false);
        }
        let previous = self.confirmed.ok_or(CompositionError::Context)?;
        if status != "Applied" && status != "APPLIED"
            || cause.is_empty()
            || cause.len() > 1024
            || revision.is_some_and(|v| v <= previous.revision)
        {
            return Err(CompositionError::Result);
        }
        let result = CommitResult {
            cause: Sha256::digest(cause.as_bytes()).into(),
            revision,
        };
        let expected = self.expected.as_mut().ok_or(CompositionError::Result)?;
        if expected.result.is_some_and(|old| old != result) {
            return Err(CompositionError::Result);
        }
        expected.result = Some(result);
        self.receipt = None;
        Ok(true)
    }
    /// Only the exact focus-surface capture token can confirm the measured clock
    /// profile. Foreign, coalesced-away or coincidental source text cannot pass.
    pub fn observe(
        &mut self,
        state: &State,
        binding: &CommandBinding,
    ) -> Result<bool, CompositionError> {
        let Some(previous) = self.confirmed else {
            return Ok(false);
        };
        let actual =
            editing_context(state, previous.target, binding).ok_or(CompositionError::Context)?;
        let Some(expected) = &self.expected else {
            return if actual == previous {
                Ok(false)
            } else {
                Err(CompositionError::Context)
            };
        };
        if (
            actual.epoch,
            actual.view,
            actual.lease,
            actual.generation,
            actual.target,
            actual.text,
            actual.focus_generation,
        ) != (
            previous.epoch,
            previous.view,
            previous.lease,
            previous.generation,
            previous.target,
            previous.text,
            previous.focus_generation,
        ) {
            return Err(CompositionError::Context);
        }
        let Record::Text(model) = state
            .records
            .get(&actual.text)
            .ok_or(CompositionError::Context)?
        else {
            return Err(CompositionError::Context);
        };
        let content = model.content.as_deref().ok_or(CompositionError::Context)?;
        let unchanged = actual == previous && content == expected.before;
        let (text_delta, record_delta) = self.policy.deltas(
            &expected.before,
            &expected.after,
            previous.selection,
            expected.selection,
        );
        let exact = content == expected.after
            && actual.selection == expected.selection
            && Some(actual.text_version) == previous.text_version.checked_add(text_delta)
            && Some(actual.text_record_version)
                == previous.text_record_version.checked_add(record_delta)
            && (actual.target_version == previous.target_version
                || Some(actual.target_version) == previous.target_version.checked_add(1));
        if !unchanged && !exact {
            return Err(CompositionError::Context);
        }
        let Some(result) = expected.result else {
            return Ok(false);
        };
        if state.revision <= previous.revision
            || result.revision.is_some_and(|v| state.revision < v)
        {
            return Ok(false);
        }
        if !exact {
            return if result.revision.is_none() && unchanged {
                Ok(false)
            } else {
                Err(CompositionError::Context)
            };
        }
        if focus_surface(state, actual)? != expected.surface {
            return Err(CompositionError::Context);
        }
        let Record::Surface(surface) = state
            .records
            .get(&expected.surface)
            .ok_or(CompositionError::Context)?
        else {
            return Err(CompositionError::Context);
        };
        if !surface
            .source_capture_tokens
            .iter()
            .any(|token| <[u8; 32]>::from(Sha256::digest(token)) == result.cause)
        {
            return if unchanged && result.revision.is_none() {
                Ok(false)
            } else {
                Err(CompositionError::Result)
            };
        }
        self.confirmed = Some(actual);
        self.expected = None;
        Ok(true)
    }
}
fn operation_bytes(operation: &CompositionOperation) -> usize {
    match operation {
        CompositionOperation::UpdatePreedit { text, .. }
        | CompositionOperation::CommitText { text } => text.len(),
        _ => 0,
    }
}
fn focus_surface(state: &State, context: EditingContext) -> Result<u64, CompositionError> {
    let reference = state
        .records
        .values()
        .find_map(|record| match record {
            Record::Session(session)
                if session.focus
                    == Some(ObjectRef {
                        kind: ObjectKind::Semantic,
                        id: context.target,
                    })
                    && session.focus_generation == context.focus_generation =>
            {
                session.focus_surface
            }
            _ => None,
        })
        .ok_or(CompositionError::Context)?;
    if reference.kind != ObjectKind::Surface {
        return Err(CompositionError::Context);
    }
    match state.records.get(&reference.id) {
        Some(Record::Surface(surface))
            if surface.mapped && surface.capture_consistency == CaptureConsistency::Coherent =>
        {
            Ok(reference.id)
        }
        _ => Err(CompositionError::Context),
    }
}
