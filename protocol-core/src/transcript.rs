//! Bounded, principal/view-scoped protocol diagnostics. Ordinary recording keeps
//! only hashes and metadata. Exact payload retention requires an explicit trusted
//! diagnostic policy; handshake credentials must never be sent to this recorder.
use crate::resources::{MemoryBudget, MemoryError, MemoryReservation, ResourceContext};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::fmt;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    HostToClient,
    ClientToHost,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingPolicy {
    MetadataOnly,
    AuthorizedDiagnosticBytes,
}
#[derive(Clone, Debug)]
pub struct TranscriptLimits {
    pub records: usize,
    pub message_bytes: usize,
    pub retained_bytes: usize,
}
impl Default for TranscriptLimits {
    fn default() -> Self {
        Self {
            records: 4096,
            message_bytes: 1 << 20,
            retained_bytes: 8 << 20,
        }
    }
}
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TranscriptError {
    #[error("transcript principal/view binding differs or was revoked")]
    Unauthorized,
    #[error("transcript retention ceiling exceeded")]
    Limit,
    #[error(transparent)]
    Memory(#[from] MemoryError),
}
pub struct RecordedMessage {
    pub sequence: u64,
    pub direction: Direction,
    pub kind: u32,
    pub wire_bytes: usize,
    pub digest: [u8; 32],
    exact: Option<Box<[u8]>>,
    _memory: MemoryReservation,
}
impl fmt::Debug for RecordedMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordedMessage")
            .field("sequence", &self.sequence)
            .field("direction", &self.direction)
            .field("kind", &self.kind)
            .field("wire_bytes", &self.wire_bytes)
            .field("digest", &self.digest)
            .field("exact_bytes_retained", &self.exact.is_some())
            .finish()
    }
}
impl RecordedMessage {
    /// Available only under the constructor's trusted diagnostic authorization.
    pub fn exact_bytes(&self) -> Option<&[u8]> {
        self.exact.as_deref()
    }
}
pub struct TranscriptRecorder {
    context: ResourceContext,
    policy: RecordingPolicy,
    limits: TranscriptLimits,
    budget: MemoryBudget,
    entries: VecDeque<RecordedMessage>,
    charged: usize,
    revoked: bool,
}
impl TranscriptRecorder {
    pub fn new(
        context: ResourceContext,
        policy: RecordingPolicy,
        limits: TranscriptLimits,
        budget: MemoryBudget,
    ) -> Result<Self, TranscriptError> {
        if limits.records == 0 || limits.message_bytes == 0 || limits.retained_bytes == 0 {
            return Err(TranscriptError::Limit);
        }
        Ok(Self {
            context,
            policy,
            limits,
            budget,
            entries: VecDeque::new(),
            charged: 0,
            revoked: false,
        })
    }
    fn authorize(&self, context: ResourceContext) -> Result<(), TranscriptError> {
        if self.revoked || context != self.context {
            Err(TranscriptError::Unauthorized)
        } else {
            Ok(())
        }
    }
    /// Call only after authenticating and authorizing the message for this view.
    /// `kind` is a validated post-handshake message type, never credential data.
    pub fn record(
        &mut self,
        context: ResourceContext,
        direction: Direction,
        kind: u32,
        bytes: &[u8],
    ) -> Result<u64, TranscriptError> {
        self.authorize(context)?;
        if kind == 0
            || bytes.len() > self.limits.message_bytes
            || self.entries.len() >= self.limits.records
        {
            return Err(TranscriptError::Limit);
        }
        let retain = self.policy == RecordingPolicy::AuthorizedDiagnosticBytes;
        // Conservatively covers VecDeque spare slots and all record metadata.
        let charge = 2048usize
            .checked_add(if retain { bytes.len() } else { 0 })
            .ok_or(TranscriptError::Limit)?;
        let total = self
            .charged
            .checked_add(charge)
            .filter(|n| *n <= self.limits.retained_bytes)
            .ok_or(TranscriptError::Limit)?;
        let memory = self.budget.reserve(charge)?;
        let sequence = u64::try_from(self.entries.len())
            .ok()
            .and_then(|v| v.checked_add(1))
            .ok_or(TranscriptError::Limit)?;
        let digest = Sha256::digest(bytes).into();
        let exact = retain.then(|| bytes.to_vec().into_boxed_slice());
        self.entries.push_back(RecordedMessage {
            sequence,
            direction,
            kind,
            wire_bytes: bytes.len(),
            digest,
            exact,
            _memory: memory,
        });
        self.charged = total;
        Ok(sequence)
    }
    pub fn entries(
        &self,
        context: ResourceContext,
    ) -> Result<impl Iterator<Item = &RecordedMessage>, TranscriptError> {
        self.authorize(context)?;
        Ok(self.entries.iter())
    }
    pub fn retained_bytes(&self) -> usize {
        self.charged
    }
    /// Irreversible scope revocation discards all retained diagnostic bodies and
    /// allocations. Previously copied authorized data cannot be recalled.
    pub fn revoke(&mut self) {
        self.entries = VecDeque::new();
        self.charged = 0;
        self.revoked = true;
    }
}
