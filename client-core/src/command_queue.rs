//! Bounded exact commands retained across ambiguous socket failure. Resending
//! these bytes can recover a ledger outcome, never invent a new command effect.
use rui_protocol_core::{
    commands::{CommandFamily, Id},
    resources::{MemoryBudget, MemoryReservation},
};
use std::collections::VecDeque;
use thiserror::Error;
pub const MAX_PENDING_COMMANDS: usize = 32;
const ENTRY_OVERHEAD: usize = 8192;
#[derive(Debug, Error, PartialEq, Eq)]
pub enum QueueError {
    #[error("pending command queue or view memory limit reached")]
    Limit,
    #[error("command encoding disagrees with its reserved size")]
    Encoding,
    #[error("command sequence exhausted")]
    Sequence,
    #[error("new receiver limits cannot recover retained commands")]
    IncompatibleLimits,
}
pub struct QueuedCommand {
    sequence: u64,
    family: CommandFamily,
    bytes: Vec<u8>,
    _memory: MemoryReservation,
}
impl QueuedCommand {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn family(&self) -> CommandFamily {
        self.family
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
impl Drop for QueuedCommand {
    fn drop(&mut self) {
        self.bytes.fill(0);
    }
}
impl std::fmt::Debug for QueuedCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueuedCommand")
            .field("sequence", &self.sequence)
            .field("family", &self.family)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}
pub struct CommandQueue {
    epoch: Id,
    disposed: bool,
    next: u64,
    maximum: usize,
    budget: MemoryBudget,
    queue: VecDeque<QueuedCommand>,
    _memory: MemoryReservation,
}
fn metadata_bytes() -> usize {
    MAX_PENDING_COMMANDS * std::mem::size_of::<QueuedCommand>() + ENTRY_OVERHEAD
}
fn charge(bytes: usize) -> Result<usize, QueueError> {
    bytes
        .checked_mul(2)
        .and_then(|n| n.checked_add(ENTRY_OVERHEAD))
        .ok_or(QueueError::Limit)
}
impl CommandQueue {
    pub fn new(epoch: Id, budget: MemoryBudget, maximum: usize) -> Result<Self, QueueError> {
        if maximum == 0 {
            return Err(QueueError::Limit);
        }
        let memory = budget
            .reserve(metadata_bytes())
            .map_err(|_| QueueError::Limit)?;
        let mut queue = VecDeque::new();
        queue
            .try_reserve_exact(MAX_PENDING_COMMANDS)
            .map_err(|_| QueueError::Limit)?;
        Ok(Self {
            epoch,
            disposed: false,
            next: 1,
            maximum,
            budget,
            queue,
            _memory: memory,
        })
    }
    pub fn next_sequence(&self) -> u64 {
        self.next
    }
    /// Authenticated browser host high-water mark. Pending exact bytes survive.
    pub fn advance_sequence(&mut self, next: u64) -> Result<(), QueueError> {
        if next == 0 || self.disposed {
            return Err(QueueError::Sequence);
        }
        self.next = self.next.max(next);
        Ok(())
    }
    pub fn len(&self) -> usize {
        self.queue.len()
    }
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
    pub fn commands(&self) -> impl Iterator<Item = &QueuedCommand> {
        self.queue.iter()
    }
    pub fn latest(&self) -> Option<&QueuedCommand> {
        self.queue.back()
    }
    /// Trusted encoder writes into a pre-reserved buffer. The retained command,
    /// its send-time copy and metadata are charged before invoking the encoder.
    pub fn encode(
        &mut self,
        family: CommandFamily,
        length: usize,
        encode: impl FnOnce(&mut Vec<u8>) -> Result<(), ()>,
    ) -> Result<u64, QueueError> {
        if self.disposed || length > self.maximum || self.queue.len() == MAX_PENDING_COMMANDS {
            return Err(QueueError::Limit);
        }
        let next = self.next.checked_add(1).ok_or(QueueError::Sequence)?;
        let memory = self
            .budget
            .reserve(charge(length)?)
            .map_err(|_| QueueError::Limit)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| QueueError::Limit)?;
        encode(&mut bytes).map_err(|_| QueueError::Encoding)?;
        if bytes.len() != length || bytes.capacity() > length {
            return Err(QueueError::Encoding);
        }
        let sequence = self.next;
        self.queue.push_back(QueuedCommand {
            sequence,
            family,
            bytes,
            _memory: memory,
        });
        self.next = next;
        Ok(sequence)
    }
    pub fn clear(&mut self) {
        self.disposed = true;
        self.queue = VecDeque::new();
        self._memory
            .resize(0)
            .expect("shrinking a reservation cannot fail");
    }
    /// Admit caller-encoded native commands without changing their original
    /// sequence/family/bytes. A conflicting retry is sent to the host for its
    /// authoritative rejection, but cannot replace the retained original.
    pub fn retain_exact(
        &mut self,
        sequence: u64,
        family: CommandFamily,
        bytes: &[u8],
    ) -> Result<(), QueueError> {
        if self.disposed || sequence == 0 {
            return Err(QueueError::Sequence);
        }
        if self.queue.iter().any(|c| c.sequence == sequence) {
            return Ok(());
        }
        if bytes.len() > self.maximum || self.queue.len() == MAX_PENDING_COMMANDS {
            return Err(QueueError::Limit);
        }
        let next = sequence.saturating_add(1);
        let memory = self
            .budget
            .reserve(charge(bytes.len())?)
            .map_err(|_| QueueError::Limit)?;
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(bytes.len())
            .map_err(|_| QueueError::Limit)?;
        owned.extend_from_slice(bytes);
        self.queue.push_back(QueuedCommand {
            sequence,
            family,
            bytes: owned,
            _memory: memory,
        });
        self.next = self.next.max(next);
        Ok(())
    }
    /// Ambiguous/nonterminal results retain exact bytes for explicit recovery.
    pub fn result(&mut self, sequence: u64, status: &str) -> bool {
        let status = status.to_ascii_lowercase();
        let dispatched = status == "dispatched"
            && self
                .queue
                .iter()
                .find(|c| c.sequence == sequence)
                .is_some_and(|c| match c.family {
                    CommandFamily::RawInput => true,
                    CommandFamily::TextComposition => {
                        use prost::Message;
                        use rui_protocol_core::wire::proto::{
                            text_composition::Operation, TextComposition,
                        };
                        TextComposition::decode(c.bytes.as_slice())
                            .ok()
                            .is_some_and(|c| {
                                matches!(
                                    c.operation,
                                    Some(
                                        Operation::Begin(_)
                                            | Operation::UpdatePreedit(_)
                                            | Operation::Cancel(_)
                                    )
                                )
                            })
                    }
                    CommandFamily::Semantic | CommandFamily::Resize => false,
                });
        if !dispatched && !matches!(status.as_str(), "applied" | "rejected" | "failed") {
            return false;
        }
        self.complete(sequence)
    }
    /// A matching authenticated terminal result retires only that exact request.
    pub fn complete(&mut self, sequence: u64) -> bool {
        if let Some(index) = self.queue.iter().position(|c| c.sequence == sequence) {
            self.queue.remove(index);
            true
        } else {
            false
        }
    }
    /// A distinct confirmed view may use a new memory budget. Same-epoch
    /// sequence and bytes survive; a new epoch retires them and starts at one.
    /// Failed reservations leave the old owner intact.
    pub fn rebind(
        &mut self,
        epoch: Id,
        budget: MemoryBudget,
        maximum: usize,
    ) -> Result<bool, QueueError> {
        if maximum == 0 {
            return Err(QueueError::Limit);
        }
        if epoch != self.epoch {
            let replacement = Self::new(epoch, budget, maximum)?;
            *self = replacement;
            return Ok(true);
        }
        if self.queue.iter().any(|c| c.bytes.len() > maximum) {
            return Err(QueueError::IncompatibleLimits);
        }
        let memory = budget
            .reserve(metadata_bytes())
            .map_err(|_| QueueError::Limit)?;
        let mut charges = Vec::with_capacity(self.queue.len());
        for command in &self.queue {
            charges.push(
                budget
                    .reserve(charge(command.bytes.len())?)
                    .map_err(|_| QueueError::Limit)?,
            );
        }
        for (command, allocation) in self.queue.iter_mut().zip(charges) {
            command._memory = allocation;
        }
        self.budget = budget;
        self.maximum = maximum;
        self._memory = memory;
        self.disposed = false;
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;
    use rui_protocol_core::wire::proto::{self as p, text_composition::Operation};
    #[test]
    fn only_confirmed_native_dispatch_receipts_retire_without_applied() {
        let budget = MemoryBudget::new(1 << 20);
        let mut q = CommandQueue::new([1; 16], budget.clone(), 4096).unwrap();
        let raw = p::RawInputBatch {
            header: Some(p::CommandHeader {
                command_seq: 1,
                ..Default::default()
            }),
            ..Default::default()
        }
        .encode_to_vec();
        q.retain_exact(1, CommandFamily::RawInput, &raw).unwrap();
        assert!(!q.result(1, "Unknown"));
        assert_eq!(q.latest().unwrap().bytes(), raw);
        assert!(q.result(1, "Dispatched"));
        for (sequence, operation, terminal) in [
            (2, Operation::Begin(p::ScalarRange::default()), true),
            (
                3,
                Operation::UpdatePreedit(p::Preedit {
                    text: "private local preedit".into(),
                    ..Default::default()
                }),
                true,
            ),
            (4, Operation::Cancel(p::EmptyArguments::default()), true),
            (
                5,
                Operation::CommitText(p::CommitText { text: "x".into() }),
                false,
            ),
        ] {
            let wire = p::TextComposition {
                header: Some(p::CommandHeader {
                    command_seq: sequence,
                    ..Default::default()
                }),
                operation: Some(operation),
                ..Default::default()
            }
            .encode_to_vec();
            q.retain_exact(sequence, CommandFamily::TextComposition, &wire)
                .unwrap();
            assert_eq!(q.result(sequence, "Dispatched"), terminal);
        }
        assert_eq!(q.len(), 1);
        for status in [
            "Accepted",
            "OutcomeUnknown",
            "OutcomeExpired",
            "future-status",
        ] {
            assert!(!q.result(5, status));
        }
        assert!(q.result(5, "Applied"));
        // A conflicting retry never replaces the original family/bytes.
        q.retain_exact(6, CommandFamily::Semantic, b"original")
            .unwrap();
        q.retain_exact(6, CommandFamily::Resize, b"replacement")
            .unwrap();
        assert_eq!(q.latest().unwrap().family(), CommandFamily::Semantic);
        assert_eq!(q.latest().unwrap().bytes(), b"original");
        assert!(!q.result(6, "Dispatched"));
        q.clear();
        assert_eq!(budget.used(), 0);
        assert_eq!(
            q.encode(CommandFamily::Semantic, 1, |_| Ok(())),
            Err(QueueError::Limit)
        );
    }
}
