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
        if length > self.maximum || self.queue.len() == MAX_PENDING_COMMANDS {
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
        Ok(false)
    }
}
