//! Memory-only, principal/session scoped resource reception.
//!
//! Authentication and authorization decisions are supplied by a trusted broker;
//! a digest never grants access. This module performs no external I/O.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use thiserror::Error;

pub type ResourceDigest = [u8; 32];
pub type ViewId = [u8; 16];
pub type PublicationId = [u8; 16];

const RESOURCE_DOMAIN: &[u8] = b"RUI/0.1/resource\0";
// BTree nodes reserve multiple slots even for the first insertion. Charges
// cover sparsely occupied root nodes, not only the size of their live keys.
// A view includes slack for its nested authorization map; a pin owner includes
// slack for its nested digest set. The per-key charges cover subsequent nodes.
const ENTRY_MEMORY: usize = 4096;
const VIEW_MEMORY: usize = 2048;
const AUTHORIZATION_MEMORY: usize = 256;
const PIN_OWNER_MEMORY: usize = 2048;
const PIN_REFERENCE_MEMORY: usize = 96;
const DIGEST_SET_ROOT_MEMORY: usize = 512;

/// Shared allocation ledger. Use the same ledger for resources, publication
/// buffers and decoded replica state. Reservations must precede allocation.
#[derive(Clone, Debug)]
pub struct MemoryBudget(Arc<Mutex<MemoryUsage>>);

#[derive(Debug)]
struct MemoryUsage {
    limit: usize,
    used: usize,
    peak: usize,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MemoryError {
    #[error("memory budget exceeded")]
    Exceeded,
    #[error("memory size overflow")]
    Overflow,
}

impl MemoryBudget {
    pub fn new(limit: usize) -> Self {
        Self(Arc::new(Mutex::new(MemoryUsage {
            limit,
            used: 0,
            peak: 0,
        })))
    }

    pub fn used(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).used
    }
    /// Lifetime high-water mark of successful reservations, including scratch
    /// allocations released before callers can sample `used()`.
    pub fn peak(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).peak
    }

    pub fn limit(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).limit
    }

    pub fn reserve(&self, bytes: usize) -> Result<MemoryReservation, MemoryError> {
        let mut usage = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let next = usage.used.checked_add(bytes).ok_or(MemoryError::Overflow)?;
        if next > usage.limit {
            return Err(MemoryError::Exceeded);
        }
        usage.used = next;
        usage.peak = usage.peak.max(next);
        Ok(MemoryReservation {
            budget: self.clone(),
            bytes,
        })
    }
}

/// Keeps an allocation charged until its owner drops it. This is deliberately
/// not Clone: copying data requires its own reservation.
#[derive(Debug)]
pub struct MemoryReservation {
    budget: MemoryBudget,
    bytes: usize,
}

impl MemoryReservation {
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn resize(&mut self, bytes: usize) -> Result<(), MemoryError> {
        let mut usage = self.budget.0.lock().unwrap_or_else(|e| e.into_inner());
        let next = usage
            .used
            .checked_sub(self.bytes)
            .and_then(|n| n.checked_add(bytes))
            .ok_or(MemoryError::Overflow)?;
        if next > usage.limit {
            return Err(MemoryError::Exceeded);
        }
        usage.used = next;
        usage.peak = usage.peak.max(next);
        self.bytes = bytes;
        Ok(())
    }
}

impl Drop for MemoryReservation {
    fn drop(&mut self) {
        let mut usage = self.budget.0.lock().unwrap_or_else(|e| e.into_inner());
        usage.used -= self.bytes;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ResourceKind {
    VectorPath = 1,
    GlyphOutline = 2,
}

impl TryFrom<u32> for ResourceKind {
    type Error = ResourceError;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::VectorPath),
            2 => Ok(Self::GlyphOutline),
            _ => Err(ResourceError::UnsupportedKind),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ResourceProvenance {
    AuthoredVector = 1,
    FontOutline = 2,
}

impl TryFrom<u32> for ResourceProvenance {
    type Error = ResourceError;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::AuthoredVector),
            2 => Ok(Self::FontOutline),
            _ => Err(ResourceError::UnsupportedProvenance),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceOffer {
    pub kind: ResourceKind,
    pub version: u32,
    pub total_bytes: u64,
    pub provenance: ResourceProvenance,
    pub digest: ResourceDigest,
}

/// Hashes the exact bytes, with fixed-width big-endian descriptor fields.
pub fn resource_digest(
    kind: ResourceKind,
    version: u32,
    provenance: ResourceProvenance,
    bytes: &[u8],
) -> ResourceDigest {
    let mut digest = Sha256::new();
    digest.update(RESOURCE_DOMAIN);
    digest.update((kind as u32).to_be_bytes());
    digest.update(version.to_be_bytes());
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update((provenance as u32).to_be_bytes());
    digest.update(bytes);
    digest.finalize().into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceScope {
    pub session_id: [u8; 16],
    pub epoch: [u8; 16],
    pub principal_id: [u8; 16],
}

/// The broker must construct this from authenticated connection state, never
/// from a peer's unauthenticated principal claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceContext {
    pub scope: ResourceScope,
    pub view_id: ViewId,
}

#[derive(Clone, Debug)]
pub struct ResourceLimits {
    pub max_resource_bytes: usize,
    pub max_cache_bytes: usize,
    pub max_chunk_bytes: usize,
    pub max_chunks_per_resource: usize,
    pub max_resources: usize,
    pub max_views: usize,
    pub max_authorizations: usize,
    pub max_pin_owners: usize,
    pub max_pin_references: usize,
    pub max_vector_commands: usize,
    pub max_coordinate_abs: f64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_resource_bytes: 16 * 1024 * 1024,
            max_cache_bytes: 128 * 1024 * 1024,
            max_chunk_bytes: 64 * 1024,
            max_chunks_per_resource: 4096,
            max_resources: 4096,
            max_views: 128,
            max_authorizations: 100_000,
            max_pin_owners: 1024,
            max_pin_references: 100_000,
            max_vector_commands: 100_000,
            max_coordinate_abs: 1_000_000_000.0,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ResourceError {
    #[error("resource scope is not authorized")]
    Unauthorized,
    #[error("view is not currently authorized")]
    UnknownView,
    #[error("resource is not authorized for this view")]
    NotAuthorized,
    #[error("unsupported resource kind")]
    UnsupportedKind,
    #[error("unsupported resource provenance")]
    UnsupportedProvenance,
    #[error("invalid resource offer")]
    InvalidOffer,
    #[error("immutable resource offer conflicts with existing offer")]
    ConflictingOffer,
    #[error("resource is not available")]
    Unavailable,
    #[error("resource/chunk/entry limit exceeded")]
    Limit,
    #[error("resource chunk is not contiguous")]
    NonContiguousChunk,
    #[error("duplicate resource chunk has different exact bytes")]
    ConflictingDuplicate,
    #[error("resource is incomplete")]
    Incomplete,
    #[error("resource digest mismatch")]
    Integrity,
    #[error("invalid bounded vector resource")]
    InvalidVector,
    #[error("resource is pinned")]
    Pinned,
    #[error("resource is still receiving")]
    Receiving,
    #[error("resource allocation failed")]
    Allocation,
    #[error(transparent)]
    Memory(#[from] MemoryError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfferOutcome {
    Receiving,
    AlreadyAvailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkOutcome {
    Accepted,
    Duplicate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VectorMetadata {
    pub command_count: u32,
    pub coordinate_count: u32,
    pub contour_count: u32,
}

/// Validates a flat RVP1 stream without allocating decoded commands. Only
/// geometry is expressible: no URL, compressed data, shader or executable.
pub fn validate_vector_bytes(
    bytes: &[u8],
    limits: &ResourceLimits,
) -> Result<VectorMetadata, ResourceError> {
    if bytes.len() > limits.max_resource_bytes {
        return Err(ResourceError::Limit);
    }
    if bytes.len() < 8 || &bytes[..4] != b"RVP1" {
        return Err(ResourceError::InvalidVector);
    }
    if !limits.max_coordinate_abs.is_finite() || limits.max_coordinate_abs <= 0.0 {
        return Err(ResourceError::InvalidVector);
    }
    let count = u32::from_be_bytes(bytes[4..8].try_into().unwrap());
    if count as usize > limits.max_vector_commands || count as usize > bytes.len() - 8 {
        return Err(ResourceError::Limit);
    }
    let mut position = 8usize;
    let mut coordinates = 0u32;
    let mut contours = 0u32;
    let mut open_contour = false;
    for _ in 0..count {
        let opcode = *bytes.get(position).ok_or(ResourceError::InvalidVector)?;
        position += 1;
        let scalar_count = match opcode {
            1 => {
                contours += 1;
                open_contour = true;
                2
            }
            2 if open_contour => 2,
            3 if open_contour => 4,
            4 if open_contour => 6,
            5 if open_contour => {
                open_contour = false;
                0
            }
            _ => return Err(ResourceError::InvalidVector),
        };
        for _ in 0..scalar_count {
            let end = position
                .checked_add(8)
                .ok_or(ResourceError::InvalidVector)?;
            let raw = bytes
                .get(position..end)
                .ok_or(ResourceError::InvalidVector)?;
            let value = f64::from_bits(u64::from_be_bytes(raw.try_into().unwrap()));
            if !value.is_finite()
                || value.abs() > limits.max_coordinate_abs
                || (value == 0.0 && value.is_sign_negative())
            {
                return Err(ResourceError::InvalidVector);
            }
            position = end;
            coordinates = coordinates.checked_add(1).ok_or(ResourceError::Limit)?;
        }
    }
    if position != bytes.len() {
        return Err(ResourceError::InvalidVector);
    }
    Ok(VectorMetadata {
        command_count: count,
        coordinate_count: coordinates,
        contour_count: contours,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PinOwner {
    Live {
        view_id: ViewId,
    },
    Staged {
        view_id: ViewId,
        publication_id: PublicationId,
    },
    InFlight {
        view_id: ViewId,
        publication_id: PublicationId,
    },
}

impl PinOwner {
    pub fn view_id(&self) -> ViewId {
        match *self {
            Self::Live { view_id }
            | Self::Staged { view_id, .. }
            | Self::InFlight { view_id, .. } => view_id,
        }
    }
}

struct Grant {
    resources: BTreeMap<ResourceDigest, MemoryReservation>,
    // A live owner has a preallocated slot so staged -> live promotion never
    // allocates a BTree node after the replica has committed.
    live: Option<Pins>,
    _memory: MemoryReservation,
}

struct Pins {
    digests: BTreeSet<ResourceDigest>,
    _memory: MemoryReservation,
}

struct Entry {
    offer: ResourceOffer,
    bytes: Vec<u8>,
    spans: Vec<(usize, usize)>,
    metadata: Option<VectorMetadata>,
    poisoned: bool,
    memory: MemoryReservation,
}

/// Returned by each eviction so the caller can emit ResourceEvicted to the
/// host. Eviction never happens silently as a side effect of reception.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceEvicted {
    pub digest: ResourceDigest,
}

/// An authorized availability set whose allocation remains in the shared
/// ledger for as long as the set is retained by the caller.
#[derive(Debug)]
pub struct AvailableResources {
    digests: BTreeSet<ResourceDigest>,
    _memory: MemoryReservation,
}

impl std::ops::Deref for AvailableResources {
    type Target = BTreeSet<ResourceDigest>;
    fn deref(&self) -> &Self::Target {
        &self.digests
    }
}

pub struct ResourceCache {
    scope: ResourceScope,
    limits: ResourceLimits,
    budget: MemoryBudget,
    grants: BTreeMap<ViewId, Grant>,
    entries: BTreeMap<ResourceDigest, Entry>,
    pins: BTreeMap<PinOwner, Pins>,
    cache_bytes: usize,
}

impl ResourceCache {
    pub fn new(scope: ResourceScope, limits: ResourceLimits, budget: MemoryBudget) -> Self {
        Self {
            scope,
            limits,
            budget,
            grants: BTreeMap::new(),
            entries: BTreeMap::new(),
            pins: BTreeMap::new(),
            cache_bytes: 0,
        }
    }

    pub fn memory_budget(&self) -> MemoryBudget {
        self.budget.clone()
    }

    pub fn cache_bytes(&self) -> usize {
        self.cache_bytes
    }

    /// Trusted broker operation, not a peer request handler.
    pub fn open_view(&mut self, view_id: ViewId) -> Result<(), ResourceError> {
        if self.grants.contains_key(&view_id) {
            return Ok(());
        }
        if self.grants.len() >= self.limits.max_views {
            return Err(ResourceError::Limit);
        }
        let memory = self.budget.reserve(VIEW_MEMORY)?;
        self.grants.insert(
            view_id,
            Grant {
                resources: BTreeMap::new(),
                live: None,
                _memory: memory,
            },
        );
        Ok(())
    }

    /// Trusted broker operation. Knowing a digest is insufficient to call it
    /// on behalf of a client; the broker must verify the projection policy.
    pub fn authorize_resource(
        &mut self,
        view_id: ViewId,
        digest: ResourceDigest,
    ) -> Result<(), ResourceError> {
        let total = self
            .grants
            .values()
            .map(|g| g.resources.len())
            .sum::<usize>();
        let grant = self
            .grants
            .get_mut(&view_id)
            .ok_or(ResourceError::UnknownView)?;
        if grant.resources.contains_key(&digest) {
            return Ok(());
        }
        if total >= self.limits.max_authorizations {
            return Err(ResourceError::Limit);
        }
        grant
            .resources
            .insert(digest, self.budget.reserve(AUTHORIZATION_MEMORY)?);
        Ok(())
    }

    pub fn revoke_resource(&mut self, view_id: ViewId, digest: ResourceDigest) {
        if let Some(grant) = self.grants.get_mut(&view_id) {
            grant.resources.remove(&digest);
        }
        // Pins persist until their scene owner is explicitly discarded. This
        // prevents memory pressure from invalidating a still-live scene.
        self.purge_unowned();
    }

    pub fn close_view(&mut self, view_id: ViewId) {
        self.grants.remove(&view_id);
        self.pins.retain(|owner, _| owner.view_id() != view_id);
        self.purge_unowned();
    }

    // Disconnection/policy revocation can remove the final owner during a
    // transfer. Such a body must not survive as unreachable staging forever.
    // No remaining authorized view needs an eviction notification here.
    fn purge_unowned(&mut self) {
        let grants = &self.grants;
        let pins = &self.pins;
        let cache_bytes = &mut self.cache_bytes;
        self.entries.retain(|digest, entry| {
            let owned = grants
                .values()
                .any(|grant| grant.resources.contains_key(digest))
                || pins.values().any(|owner| owner.digests.contains(digest))
                || grants.values().any(|grant| {
                    grant
                        .live
                        .as_ref()
                        .is_some_and(|pins| pins.digests.contains(digest))
                });
            if !owned {
                *cache_bytes -= entry.offer.total_bytes as usize;
            }
            owned
        });
    }

    fn check_view(&self, context: ResourceContext) -> Result<&Grant, ResourceError> {
        if context.scope != self.scope {
            return Err(ResourceError::Unauthorized);
        }
        self.grants
            .get(&context.view_id)
            .ok_or(ResourceError::UnknownView)
    }

    fn check_resource(
        &self,
        context: ResourceContext,
        digest: ResourceDigest,
    ) -> Result<(), ResourceError> {
        let grant = self.check_view(context)?;
        if !grant.resources.contains_key(&digest) {
            return Err(ResourceError::NotAuthorized);
        }
        Ok(())
    }

    pub fn receive_offer(
        &mut self,
        context: ResourceContext,
        offer: ResourceOffer,
    ) -> Result<OfferOutcome, ResourceError> {
        self.check_resource(context, offer.digest)?;
        if offer.version != 1
            || !matches!(
                (offer.kind, offer.provenance),
                (ResourceKind::VectorPath, ResourceProvenance::AuthoredVector)
                    | (ResourceKind::GlyphOutline, ResourceProvenance::FontOutline)
            )
        {
            return Err(ResourceError::InvalidOffer);
        }
        let length = usize::try_from(offer.total_bytes).map_err(|_| ResourceError::Limit)?;
        if length < 8 || length > self.limits.max_resource_bytes {
            return Err(ResourceError::Limit);
        }
        if let Some(entry) = self.entries.get(&offer.digest) {
            if entry.offer != offer {
                return Err(ResourceError::ConflictingOffer);
            }
            return Ok(if entry.metadata.is_some() {
                OfferOutcome::AlreadyAvailable
            } else {
                OfferOutcome::Receiving
            });
        }
        let next_cache = self
            .cache_bytes
            .checked_add(length)
            .ok_or(ResourceError::Limit)?;
        if self.entries.len() >= self.limits.max_resources
            || next_cache > self.limits.max_cache_bytes
        {
            return Err(ResourceError::Limit);
        }
        if self.limits.max_chunk_bytes == 0
            || self.limits.max_chunks_per_resource == 0
            || length.div_ceil(self.limits.max_chunk_bytes) > self.limits.max_chunks_per_resource
        {
            return Err(ResourceError::Limit);
        }
        let span_count = length.min(self.limits.max_chunks_per_resource);
        let charge = span_count
            .checked_mul(std::mem::size_of::<(usize, usize)>())
            .and_then(|n| n.checked_add(length))
            .and_then(|n| n.checked_add(ENTRY_MEMORY))
            .ok_or(ResourceError::Limit)?;
        let memory = self.budget.reserve(charge)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| ResourceError::Allocation)?;
        let mut spans = Vec::new();
        spans
            .try_reserve_exact(span_count)
            .map_err(|_| ResourceError::Allocation)?;
        self.entries.insert(
            offer.digest,
            Entry {
                offer,
                bytes,
                spans,
                metadata: None,
                poisoned: false,
                memory,
            },
        );
        self.cache_bytes = next_cache;
        Ok(OfferOutcome::Receiving)
    }

    pub fn receive_chunk(
        &mut self,
        context: ResourceContext,
        digest: ResourceDigest,
        index: u32,
        data: &[u8],
    ) -> Result<ChunkOutcome, ResourceError> {
        self.check_resource(context, digest)?;
        if data.is_empty() || data.len() > self.limits.max_chunk_bytes {
            return Err(ResourceError::Limit);
        }
        let entry = self
            .entries
            .get_mut(&digest)
            .ok_or(ResourceError::Unavailable)?;
        if entry.metadata.is_some() {
            return Err(ResourceError::Receiving);
        }
        if entry.poisoned {
            return Err(ResourceError::ConflictingDuplicate);
        }
        let index = index as usize;
        if index < entry.spans.len() {
            let (start, end) = entry.spans[index];
            return if &entry.bytes[start..end] == data {
                Ok(ChunkOutcome::Duplicate)
            } else {
                entry.poisoned = true;
                Err(ResourceError::ConflictingDuplicate)
            };
        }
        if index != entry.spans.len() {
            return Err(ResourceError::NonContiguousChunk);
        }
        if index >= self.limits.max_chunks_per_resource {
            return Err(ResourceError::Limit);
        }
        let end = entry
            .bytes
            .len()
            .checked_add(data.len())
            .ok_or(ResourceError::Limit)?;
        if end > entry.offer.total_bytes as usize {
            return Err(ResourceError::Limit);
        }
        let start = entry.bytes.len();
        entry.bytes.extend_from_slice(data);
        entry.spans.push((start, end));
        Ok(ChunkOutcome::Accepted)
    }

    /// Availability is reported only after exact length/hash and safe decoding.
    /// A failed completion discards the receiving body, preserving owner pins.
    pub fn complete(
        &mut self,
        context: ResourceContext,
        digest: ResourceDigest,
    ) -> Result<VectorMetadata, ResourceError> {
        self.check_resource(context, digest)?;
        let entry = self
            .entries
            .get(&digest)
            .ok_or(ResourceError::Unavailable)?;
        if let Some(metadata) = entry.metadata {
            return Ok(metadata);
        }
        let validation = if entry.poisoned {
            Err(ResourceError::ConflictingDuplicate)
        } else if entry.bytes.len() != entry.offer.total_bytes as usize {
            Err(ResourceError::Incomplete)
        } else if resource_digest(
            entry.offer.kind,
            entry.offer.version,
            entry.offer.provenance,
            &entry.bytes,
        ) != digest
        {
            Err(ResourceError::Integrity)
        } else {
            validate_vector_bytes(&entry.bytes, &self.limits)
        };
        match validation {
            Ok(metadata) => {
                let entry = self.entries.get_mut(&digest).unwrap();
                entry.spans = Vec::new();
                entry.memory.resize(entry.bytes.capacity() + ENTRY_MEMORY)?;
                entry.metadata = Some(metadata);
                Ok(metadata)
            }
            Err(error) => {
                let entry = self.entries.remove(&digest).unwrap();
                self.cache_bytes -= entry.offer.total_bytes as usize;
                Err(error)
            }
        }
    }

    pub fn abort_transfer(
        &mut self,
        context: ResourceContext,
        digest: ResourceDigest,
    ) -> Result<(), ResourceError> {
        self.check_resource(context, digest)?;
        if let Some(entry) = self.entries.get(&digest) {
            if entry.metadata.is_some() {
                return Err(ResourceError::Pinned);
            }
            let entry = self.entries.remove(&digest).unwrap();
            self.cache_bytes -= entry.offer.total_bytes as usize;
        }
        Ok(())
    }

    pub fn read(
        &self,
        context: ResourceContext,
        digest: ResourceDigest,
    ) -> Result<&[u8], ResourceError> {
        self.check_resource(context, digest)?;
        let entry = self
            .entries
            .get(&digest)
            .ok_or(ResourceError::Unavailable)?;
        if entry.metadata.is_none() {
            return Err(ResourceError::Unavailable);
        }
        Ok(&entry.bytes)
    }

    pub fn available_for_view(
        &self,
        context: ResourceContext,
    ) -> Result<AvailableResources, ResourceError> {
        let grant = self.check_view(context)?;
        let available = || {
            grant
                .resources
                .keys()
                .filter(|digest| {
                    self.entries
                        .get(*digest)
                        .is_some_and(|entry| entry.metadata.is_some())
                })
                .copied()
        };
        let count = available().count();
        let charge = count
            .checked_mul(PIN_REFERENCE_MEMORY)
            .and_then(|bytes| {
                bytes.checked_add(if count == 0 {
                    0
                } else {
                    DIGEST_SET_ROOT_MEMORY
                })
            })
            .ok_or(ResourceError::Limit)?;
        let memory = self.budget.reserve(charge)?;
        Ok(AvailableResources {
            digests: available().collect(),
            _memory: memory,
        })
    }

    /// Missing resources may be pinned before offer/receipt; completion preserves
    /// those pins. Replacement is atomic if authorization/budget checks fail.
    pub fn replace_pins(
        &mut self,
        context: ResourceContext,
        owner: PinOwner,
        digests: &BTreeSet<ResourceDigest>,
    ) -> Result<(), ResourceError> {
        self.check_view(context)?;
        if owner.view_id() != context.view_id {
            return Err(ResourceError::Unauthorized);
        }
        for digest in digests {
            self.check_resource(context, *digest)?;
        }
        let old_pins = match owner {
            PinOwner::Live { view_id } => self.grants[&view_id].live.as_ref(),
            _ => self.pins.get(&owner),
        };
        let old_count = old_pins.map_or(0, |pins| pins.digests.len());
        let exists = old_pins.is_some();
        let live_owners = self.grants.values().filter_map(|grant| grant.live.as_ref());
        let total = self
            .pins
            .values()
            .chain(live_owners)
            .map(|pins| pins.digests.len())
            .sum::<usize>();
        if total
            .checked_sub(old_count)
            .and_then(|n| n.checked_add(digests.len()))
            .ok_or(ResourceError::Limit)?
            > self.limits.max_pin_references
            || (!digests.is_empty()
                && !exists
                && self.pins.len() + self.grants.values().filter(|g| g.live.is_some()).count()
                    >= self.limits.max_pin_owners)
        {
            return Err(ResourceError::Limit);
        }
        if digests.is_empty() {
            self.remove_pins(owner);
            self.purge_unowned();
            return Ok(());
        }
        let charge = digests
            .len()
            .checked_mul(PIN_REFERENCE_MEMORY)
            .and_then(|n| n.checked_add(PIN_OWNER_MEMORY))
            .ok_or(ResourceError::Limit)?;
        let memory = self.budget.reserve(charge)?;
        let pins = Pins {
            digests: digests.clone(),
            _memory: memory,
        };
        match owner {
            PinOwner::Live { view_id } => self.grants.get_mut(&view_id).unwrap().live = Some(pins),
            _ => {
                self.pins.insert(owner, pins);
            }
        }
        self.purge_unowned();
        Ok(())
    }

    pub fn release_pins(
        &mut self,
        context: ResourceContext,
        owner: PinOwner,
    ) -> Result<(), ResourceError> {
        self.check_view(context)?;
        if owner.view_id() != context.view_id {
            return Err(ResourceError::Unauthorized);
        }
        self.remove_pins(owner);
        self.purge_unowned();
        Ok(())
    }

    pub fn is_pinned(&self, digest: ResourceDigest) -> bool {
        self.pins
            .values()
            .chain(self.grants.values().filter_map(|grant| grant.live.as_ref()))
            .any(|pins| pins.digests.contains(&digest))
    }

    fn remove_pins(&mut self, owner: PinOwner) {
        match owner {
            PinOwner::Live { view_id } => {
                if let Some(grant) = self.grants.get_mut(&view_id) {
                    grant.live = None;
                }
            }
            _ => {
                self.pins.remove(&owner);
            }
        }
    }

    /// Receiver-only commit step. Authorization and staged pin allocation were
    /// checked before reducer commit, and serialized access prevents changes.
    /// Moving into the view's live slot cannot allocate or fail.
    pub(crate) fn promote_staged_pins(&mut self, context: ResourceContext, id: PublicationId) {
        debug_assert!(self.check_view(context).is_ok());
        let staged = self.pins.remove(&PinOwner::Staged {
            view_id: context.view_id,
            publication_id: id,
        });
        self.grants.get_mut(&context.view_id).unwrap().live = staged;
        self.purge_unowned();
    }

    pub(crate) fn available_offer(
        &self,
        context: ResourceContext,
        digest: ResourceDigest,
    ) -> Result<&ResourceOffer, ResourceError> {
        self.read(context, digest)?;
        Ok(&self.entries[&digest].offer)
    }

    /// Drop partial transfers on disconnect without discarding verified cache
    /// entries or the live scene's ownership.
    pub(crate) fn abort_receiving(&mut self) {
        let cache_bytes = &mut self.cache_bytes;
        self.entries.retain(|_, entry| {
            let complete = entry.metadata.is_some();
            if !complete {
                *cache_bytes -= entry.offer.total_bytes as usize;
            }
            complete
        });
    }

    /// Trusted renderer/cache restart notification. Owners and authorization
    /// remain; availability must be reacquired before presentation.
    pub(crate) fn lose_entries(&mut self) {
        self.entries.clear();
        self.cache_bytes = 0;
    }

    /// Release grants left by rejected/abandoned candidate work. Verified cache
    /// entries retain their grants for reuse; live/staged pins retain theirs.
    pub(crate) fn prune_unused_authorizations(&mut self, context: ResourceContext) {
        let pins = &self.pins;
        let entries = &self.entries;
        let grant = self.grants.get_mut(&context.view_id).unwrap();
        let live = &grant.live;
        grant.resources.retain(|digest, _| {
            entries
                .get(digest)
                .is_some_and(|entry| entry.metadata.is_some())
                || live
                    .as_ref()
                    .is_some_and(|owner| owner.digests.contains(digest))
                || pins.values().any(|owner| owner.digests.contains(digest))
        });
        self.purge_unowned();
    }

    pub fn evict(
        &mut self,
        context: ResourceContext,
        digest: ResourceDigest,
    ) -> Result<ResourceEvicted, ResourceError> {
        self.check_resource(context, digest)?;
        self.remove_available(digest)
    }

    fn remove_available(
        &mut self,
        digest: ResourceDigest,
    ) -> Result<ResourceEvicted, ResourceError> {
        if self.is_pinned(digest) {
            return Err(ResourceError::Pinned);
        }
        let entry = self
            .entries
            .get(&digest)
            .ok_or(ResourceError::Unavailable)?;
        if entry.metadata.is_none() {
            return Err(ResourceError::Receiving);
        }
        let entry = self.entries.remove(&digest).unwrap();
        self.cache_bytes -= entry.offer.total_bytes as usize;
        Ok(ResourceEvicted { digest })
    }

    /// Trusted cache-pressure operation returning exactly one notification at a
    /// time, so notification queues do not grow inside this cache.
    pub fn evict_one_unpinned(&mut self) -> Option<ResourceEvicted> {
        let digest = self
            .entries
            .iter()
            .find(|(digest, entry)| entry.metadata.is_some() && !self.is_pinned(**digest))
            .map(|(digest, _)| *digest)?;
        self.remove_available(digest).ok()
    }
}
