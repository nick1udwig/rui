//! Bounded exact-byte snapshot/transaction assembly. This layer never owns or
//! mutates committed replica state; the reducer validates the decoded graph.

use crate::resources::{
    ChunkOutcome, MemoryBudget, MemoryError, MemoryReservation, PublicationId, ResourceContext,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use thiserror::Error;

// One publication can occupy a sparse, multi-slot BTreeMap root by itself.
const STAGING_MEMORY: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationKind {
    Snapshot {
        revision: u64,
    },
    Transaction {
        base_revision: u64,
        new_revision: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicationBegin {
    pub publication_id: PublicationId,
    pub kind: PublicationKind,
    pub total_bytes: u64,
    pub chunk_count: u32,
    /// Number of decoded records (snapshot) or mutations (transaction). The
    /// reducer must compare this with the actual decoded payload count.
    pub item_count: u32,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicationCommit {
    pub total_bytes: u64,
    pub chunk_count: u32,
    pub item_count: u32,
    pub digest: [u8; 32],
}

impl From<&PublicationBegin> for PublicationCommit {
    fn from(begin: &PublicationBegin) -> Self {
        Self {
            total_bytes: begin.total_bytes,
            chunk_count: begin.chunk_count,
            item_count: begin.item_count,
            digest: begin.digest,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PublicationLimits {
    pub max_snapshot_bytes: usize,
    pub max_transaction_bytes: usize,
    pub max_chunk_bytes: usize,
    pub max_chunks: usize,
    pub max_items: usize,
    pub max_in_flight: usize,
}

impl Default for PublicationLimits {
    fn default() -> Self {
        Self {
            max_snapshot_bytes: 64 * 1024 * 1024,
            max_transaction_bytes: 16 * 1024 * 1024,
            max_chunk_bytes: 64 * 1024,
            max_chunks: 4096,
            max_items: 100_000,
            max_in_flight: 4,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PublicationError {
    #[error("publication context is not authorized")]
    Unauthorized,
    #[error("publication revision is invalid")]
    InvalidRevision,
    #[error("publication size/count limit exceeded")]
    Limit,
    #[error("publication ID has conflicting begin metadata")]
    ConflictingBegin,
    #[error("publication is not staged")]
    UnknownPublication,
    #[error("publication chunk is not contiguous")]
    NonContiguousChunk,
    #[error("duplicate publication chunk has different exact bytes")]
    ConflictingDuplicate,
    #[error("publication is incomplete")]
    Incomplete,
    #[error("publication commit metadata conflicts with begin")]
    ConflictingCommit,
    #[error("publication digest mismatch")]
    Integrity,
    #[error("publication allocation failed")]
    Allocation,
    #[error(transparent)]
    Memory(#[from] MemoryError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeginOutcome {
    Started,
    Duplicate,
}

struct Staging {
    begin: PublicationBegin,
    bytes: Vec<u8>,
    spans: Vec<(usize, usize)>,
    memory: MemoryReservation,
    poisoned: bool,
}

/// Owns the exact received payload and keeps it charged until dropped. Use
/// bytes() to decode it while this allocation remains in the shared budget.
#[derive(Debug)]
pub struct AssembledPublication {
    begin: PublicationBegin,
    bytes: Vec<u8>,
    _memory: MemoryReservation,
}

impl AssembledPublication {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn begin(&self) -> &PublicationBegin {
        &self.begin
    }
}

pub fn publication_digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub struct PublicationAssembler {
    context: ResourceContext,
    limits: PublicationLimits,
    budget: MemoryBudget,
    staging: BTreeMap<PublicationId, Staging>,
    authorized: bool,
}

impl PublicationAssembler {
    pub fn new(context: ResourceContext, limits: PublicationLimits, budget: MemoryBudget) -> Self {
        Self {
            context,
            limits,
            budget,
            staging: BTreeMap::new(),
            authorized: true,
        }
    }

    fn check_context(&self, context: ResourceContext) -> Result<(), PublicationError> {
        if !self.authorized || context != self.context {
            return Err(PublicationError::Unauthorized);
        }
        Ok(())
    }

    /// Revocation discards every staged buffer and cannot be undone. A changed
    /// authorization/profile requires constructing an assembler for a new view.
    pub fn revoke(&mut self) {
        self.authorized = false;
        self.staging.clear();
    }

    pub fn staged_count(&self) -> usize {
        self.staging.len()
    }

    pub fn staged_bytes(&self) -> usize {
        self.staging.values().map(|stage| stage.bytes.len()).sum()
    }

    pub fn begin(
        &mut self,
        context: ResourceContext,
        begin: PublicationBegin,
    ) -> Result<BeginOutcome, PublicationError> {
        self.check_context(context)?;
        let maximum = match begin.kind {
            PublicationKind::Snapshot { revision } if revision > 0 => {
                self.limits.max_snapshot_bytes
            }
            PublicationKind::Transaction {
                base_revision,
                new_revision,
            } if base_revision > 0 && base_revision.checked_add(1) == Some(new_revision) => {
                self.limits.max_transaction_bytes
            }
            _ => return Err(PublicationError::InvalidRevision),
        };
        let length = usize::try_from(begin.total_bytes).map_err(|_| PublicationError::Limit)?;
        let chunks = begin.chunk_count as usize;
        if length > maximum
            || chunks > self.limits.max_chunks
            || begin.item_count as usize > self.limits.max_items
            || (length == 0) != (chunks == 0)
            || chunks > length
            || self.limits.max_chunk_bytes == 0
            || (length > 0 && length.div_ceil(self.limits.max_chunk_bytes) > chunks)
        {
            return Err(PublicationError::Limit);
        }
        if let Some(stage) = self.staging.get(&begin.publication_id) {
            return if stage.begin == begin {
                Ok(BeginOutcome::Duplicate)
            } else {
                Err(PublicationError::ConflictingBegin)
            };
        }
        if self.staging.len() >= self.limits.max_in_flight {
            return Err(PublicationError::Limit);
        }
        let charge = chunks
            .checked_mul(std::mem::size_of::<(usize, usize)>())
            .and_then(|n| n.checked_add(length))
            .and_then(|n| n.checked_add(STAGING_MEMORY))
            .ok_or(PublicationError::Limit)?;
        let memory = self.budget.reserve(charge)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| PublicationError::Allocation)?;
        let mut spans = Vec::new();
        spans
            .try_reserve_exact(chunks)
            .map_err(|_| PublicationError::Allocation)?;
        self.staging.insert(
            begin.publication_id,
            Staging {
                begin,
                bytes,
                spans,
                memory,
                poisoned: false,
            },
        );
        Ok(BeginOutcome::Started)
    }

    pub fn push_chunk(
        &mut self,
        context: ResourceContext,
        id: PublicationId,
        index: u32,
        data: &[u8],
    ) -> Result<ChunkOutcome, PublicationError> {
        self.check_context(context)?;
        if data.is_empty() || data.len() > self.limits.max_chunk_bytes {
            return Err(PublicationError::Limit);
        }
        let stage = self
            .staging
            .get_mut(&id)
            .ok_or(PublicationError::UnknownPublication)?;
        if stage.poisoned {
            return Err(PublicationError::ConflictingDuplicate);
        }
        let index = index as usize;
        if index < stage.spans.len() {
            let (start, end) = stage.spans[index];
            return if &stage.bytes[start..end] == data {
                Ok(ChunkOutcome::Duplicate)
            } else {
                stage.poisoned = true;
                Err(PublicationError::ConflictingDuplicate)
            };
        }
        if index != stage.spans.len() {
            return Err(PublicationError::NonContiguousChunk);
        }
        let end = stage
            .bytes
            .len()
            .checked_add(data.len())
            .ok_or(PublicationError::Limit)?;
        if index >= stage.begin.chunk_count as usize || end > stage.begin.total_bytes as usize {
            return Err(PublicationError::Limit);
        }
        // Neither extend nor push can grow capacity: the complete declared
        // payload and chunk metadata were reserved before accepting Begin.
        let start = stage.bytes.len();
        stage.bytes.extend_from_slice(data);
        stage.spans.push((start, end));
        Ok(ChunkOutcome::Accepted)
    }

    /// Any failed Commit discards that staging allocation. No partial bytes are
    /// returned and no committed state is touched.
    pub fn finish(
        &mut self,
        context: ResourceContext,
        id: PublicationId,
        commit: PublicationCommit,
    ) -> Result<AssembledPublication, PublicationError> {
        self.check_context(context)?;
        let mut stage = self
            .staging
            .remove(&id)
            .ok_or(PublicationError::UnknownPublication)?;
        if stage.poisoned {
            return Err(PublicationError::ConflictingDuplicate);
        }
        if PublicationCommit::from(&stage.begin) != commit {
            return Err(PublicationError::ConflictingCommit);
        }
        if stage.bytes.len() != stage.begin.total_bytes as usize
            || stage.spans.len() != stage.begin.chunk_count as usize
        {
            return Err(PublicationError::Incomplete);
        }
        if publication_digest(&stage.bytes) != commit.digest {
            return Err(PublicationError::Integrity);
        }
        stage.spans = Vec::new();
        stage.memory.resize(stage.bytes.len() + STAGING_MEMORY)?;
        Ok(AssembledPublication {
            begin: stage.begin,
            bytes: stage.bytes,
            _memory: stage.memory,
        })
    }

    pub fn abort(
        &mut self,
        context: ResourceContext,
        id: PublicationId,
    ) -> Result<bool, PublicationError> {
        self.check_context(context)?;
        Ok(self.staging.remove(&id).is_some())
    }

    pub fn abort_all(&mut self, context: ResourceContext) -> Result<(), PublicationError> {
        self.check_context(context)?;
        self.staging.clear();
        Ok(())
    }
}
