//! Serialized, per-view receiver coordination. The constructor and resource
//! grants are trusted broker inputs; receive methods handle untrusted peer data.
//! This module does not authenticate, perform I/O, render, or verify local fonts.

use crate::model::{
    ApplyOutcome, CursorAppearance, GlyphFont, Limits, ModelError, Primitive, Record, Replica,
    State,
};
use crate::publication::{
    publication_digest, BeginOutcome, PublicationAssembler, PublicationBegin, PublicationCommit,
    PublicationError, PublicationKind, PublicationLimits,
};
use crate::resources::{
    ChunkOutcome, MemoryBudget, MemoryError, MemoryReservation, OfferOutcome, PinOwner,
    PublicationId, ResourceCache, ResourceContext, ResourceDigest, ResourceError, ResourceEvicted,
    ResourceKind, ResourceLimits, ResourceOffer,
};
use crate::session::ReceiverLimits;
use crate::wire::{self, proto, WireError};
use std::collections::BTreeSet;
use thiserror::Error;

/// Local implementation ceilings; negotiated ceilings can only lower them.
#[derive(Clone, Debug, Default)]
pub struct ReceiverConfig {
    pub model: Limits,
    pub publication: PublicationLimits,
    pub resources: ResourceLimits,
}

#[derive(Debug, Error, PartialEq)]
pub enum ReceiverError {
    #[error("invalid receiver binding or negotiated limits")]
    InvalidBinding,
    #[error("receiver is closed or its authorization was revoked")]
    Closed,
    #[error("receiver is disconnected")]
    Disconnected,
    #[error("message does not match the authenticated session, epoch, principal, and view")]
    Unauthorized,
    #[error("a different publication is already in flight")]
    Busy,
    #[error("invalid publication or resource identifier")]
    InvalidId,
    #[error("resource is outside the live and pending resource closure")]
    NotNeeded,
    #[error(
        "glyph rendering requires verified local fonts or an implemented outline-to-glyph mapping"
    )]
    UnsupportedFont,
    #[error("resource descriptor does not match its scene use")]
    ResourceKind,
    #[error("revision is not the current usable scene")]
    NotPresentable,
    #[error(transparent)]
    Publication(#[from] PublicationError),
    #[error(transparent)]
    Resource(#[from] ResourceError),
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error(transparent)]
    Memory(#[from] MemoryError),
}

/// A retained outbound request remains charged to the receiver's ledger until
/// dropped, including after close. It cannot be cloned or mutated.
#[derive(Debug)]
pub struct NeededResources {
    digests: BTreeSet<ResourceDigest>,
    _memory: MemoryReservation,
    _reply_quota: MemoryReservation,
}
impl std::ops::Deref for NeededResources {
    type Target = BTreeSet<ResourceDigest>;
    fn deref(&self) -> &Self::Target {
        &self.digests
    }
}

#[derive(Debug)]
pub enum ReceiveOutcome {
    /// Usable resource closure and replica state committed together.
    Applied {
        revision: u64,
    },
    Duplicate {
        revision: u64,
    },
    NeedResources {
        publication_id: PublicationId,
        revision: u64,
        missing: NeededResources,
    },
}

/// Returned only by the trusted renderer completion callback. It records the
/// caller's presentation evidence; the receiver cannot prove rendering occurred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentedAck {
    pub revision: u64,
}

struct ActivePublication {
    begin: PublicationBegin,
    pending: bool,
}
struct Inner {
    replica: Replica,
    cache: ResourceCache,
    assembler: PublicationAssembler,
    active: Option<ActivePublication>,
    _memory: MemoryReservation,
}

/// Owns every mutable reception layer. A single publication may be assembled or
/// waiting for resources at a time. Peer messages cannot manufacture availability
/// or mutate resource owners, policy, or replica internals.
pub struct ViewReceiver {
    context: ResourceContext,
    budget: MemoryBudget,
    reply_quota: MemoryBudget,
    inner: Option<Inner>,
    connected: bool,
}

impl ViewReceiver {
    /// `context` comes from authenticated connection state and authorized view
    /// policy, never a peer principal claim. `negotiated` must be the negotiated
    /// minima, not the peer's unverified preferences.
    pub fn new(
        context: ResourceContext,
        negotiated: ReceiverLimits,
        mut local: ReceiverConfig,
    ) -> Result<Self, ReceiverError> {
        if [
            context.scope.session_id,
            context.scope.epoch,
            context.scope.principal_id,
            context.view_id,
        ]
        .contains(&[0; 16])
            || negotiated.negotiate(&negotiated).is_err()
        {
            return Err(ReceiverError::InvalidBinding);
        }
        local.model.max_envelope_bytes = local
            .model
            .max_envelope_bytes
            .min(negotiated.envelope_bytes);
        local.model.max_snapshot_bytes = local
            .model
            .max_snapshot_bytes
            .min(negotiated.snapshot_bytes);
        local.model.max_transaction_bytes = local
            .model
            .max_transaction_bytes
            .min(negotiated.transaction_bytes);
        local.model.max_memory_bytes = local
            .model
            .max_memory_bytes
            .min(negotiated.view_memory_bytes);
        local.model.max_records = local.model.max_records.min(negotiated.records);
        local.model.max_depth = local.model.max_depth.min(negotiated.ownership_depth);
        local.model.max_retired = local.model.max_retired.min(negotiated.records);
        local.publication.max_snapshot_bytes = local
            .publication
            .max_snapshot_bytes
            .min(local.model.max_snapshot_bytes);
        local.publication.max_transaction_bytes = local
            .publication
            .max_transaction_bytes
            .min(local.model.max_transaction_bytes);
        local.publication.max_chunk_bytes = local
            .publication
            .max_chunk_bytes
            .min(negotiated.chunk_bytes);
        local.publication.max_items = local.publication.max_items.min(local.model.max_records);
        local.publication.max_in_flight = 1;
        local.resources.max_resource_bytes = local
            .resources
            .max_resource_bytes
            .min(negotiated.resource_bytes);
        local.resources.max_cache_bytes = local
            .resources
            .max_cache_bytes
            .min(local.model.max_memory_bytes);
        local.resources.max_chunk_bytes =
            local.resources.max_chunk_bytes.min(negotiated.chunk_bytes);
        local.resources.max_views = local.resources.max_views.min(1);
        local.resources.max_coordinate_abs = local
            .resources
            .max_coordinate_abs
            .min(local.model.max_geometry);
        if [
            local.model.max_envelope_bytes,
            local.model.max_snapshot_bytes,
            local.model.max_transaction_bytes,
            local.model.max_memory_bytes,
            local.model.max_records,
            local.model.max_depth,
            local.publication.max_chunk_bytes,
            local.publication.max_chunks,
            local.resources.max_resource_bytes,
            local.resources.max_chunk_bytes,
            local.resources.max_chunks_per_resource,
            local.resources.max_views,
        ]
        .contains(&0)
            || !local.resources.max_coordinate_abs.is_finite()
            || local.resources.max_coordinate_abs <= 0.0
        {
            return Err(ReceiverError::InvalidBinding);
        }
        let budget = MemoryBudget::new(local.model.max_memory_bytes);
        let memory = budget.reserve(4096)?;
        let mut cache = ResourceCache::new(context.scope, local.resources, budget.clone());
        cache.open_view(context.view_id)?;
        let replica = Replica::with_memory_budget(local.model, budget.clone());
        let assembler = PublicationAssembler::new(context, local.publication, budget.clone());
        Ok(Self {
            context,
            budget,
            reply_quota: MemoryBudget::new(
                negotiated
                    .outbound_queue_bytes
                    .min(negotiated.view_memory_bytes),
            ),
            inner: Some(Inner {
                replica,
                cache,
                assembler,
                active: None,
                _memory: memory,
            }),
            connected: true,
        })
    }

    fn check(&self, context: ResourceContext) -> Result<(), ReceiverError> {
        if self.inner.is_none() {
            return Err(ReceiverError::Closed);
        }
        if context != self.context {
            return Err(ReceiverError::Unauthorized);
        }
        if !self.connected {
            return Err(ReceiverError::Disconnected);
        }
        Ok(())
    }
    fn inner(&self) -> Result<&Inner, ReceiverError> {
        self.inner.as_ref().ok_or(ReceiverError::Closed)
    }
    pub fn state(&self) -> Result<&State, ReceiverError> {
        Ok(self.inner()?.replica.state())
    }
    pub fn memory_used(&self) -> usize {
        self.budget.used()
    }
    pub fn memory_limit(&self) -> usize {
        self.budget.limit()
    }
    pub fn cache_bytes(&self) -> Result<usize, ReceiverError> {
        Ok(self.inner()?.cache.cache_bytes())
    }
    pub fn presented_revision(&self) -> Result<Option<u64>, ReceiverError> {
        Ok(self.inner()?.replica.presented_revision())
    }
    pub fn has_pending_publication(&self) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|inner| inner.active.is_some())
    }

    /// Trusted policy grant, not a peer message handler. Grants alone never
    /// make a resource available and do not permit unsolicited resource bodies.
    pub fn authorize_resource(&mut self, digest: ResourceDigest) -> Result<(), ReceiverError> {
        self.check(self.context)?;
        if digest == [0; 32] {
            return Err(ReceiverError::InvalidId);
        }
        self.inner
            .as_mut()
            .unwrap()
            .cache
            .authorize_resource(self.context.view_id, digest)?;
        Ok(())
    }

    pub fn begin_publication(
        &mut self,
        context: ResourceContext,
        begin: PublicationBegin,
    ) -> Result<BeginOutcome, ReceiverError> {
        self.check(context)?;
        if begin.publication_id == [0; 16] {
            return Err(ReceiverError::InvalidId);
        }
        let inner = self.inner.as_mut().unwrap();
        if let Some(active) = &inner.active {
            if active.begin == begin {
                return Ok(BeginOutcome::Duplicate);
            }
            return Err(if active.begin.publication_id == begin.publication_id {
                PublicationError::ConflictingBegin.into()
            } else {
                ReceiverError::Busy
            });
        }
        let outcome = match inner.assembler.begin(context, begin.clone()) {
            Ok(outcome) => outcome,
            Err(error) => {
                inner.cache.prune_unused_authorizations(context);
                return Err(error.into());
            }
        };
        inner.active = Some(ActivePublication {
            begin,
            pending: false,
        });
        Ok(outcome)
    }
    pub fn publication_chunk(
        &mut self,
        context: ResourceContext,
        id: PublicationId,
        index: u32,
        bytes: &[u8],
    ) -> Result<ChunkOutcome, ReceiverError> {
        self.check(context)?;
        let inner = self.inner.as_mut().unwrap();
        if inner.active.as_ref().is_some_and(|active| active.pending) {
            return Err(ReceiverError::Busy);
        }
        Ok(inner.assembler.push_chunk(context, id, index, bytes)?)
    }
    pub fn commit_publication(
        &mut self,
        context: ResourceContext,
        id: PublicationId,
        commit: PublicationCommit,
    ) -> Result<ReceiveOutcome, ReceiverError> {
        self.check(context)?;
        let inner = self.inner.as_mut().unwrap();
        if let Some(active) = &inner.active {
            if active.begin.publication_id != id {
                return Err(PublicationError::UnknownPublication.into());
            }
            if active.pending {
                if PublicationCommit::from(&active.begin) != commit {
                    return Err(PublicationError::ConflictingCommit.into());
                }
                return self.retry_pending();
            }
        }
        let publication = match inner.assembler.finish(context, id, commit) {
            Ok(publication) => publication,
            Err(error) => {
                inner.active = None;
                inner.cache.prune_unused_authorizations(context);
                return Err(error.into());
            }
        };
        // Empty availability forces every resourceful graph into validated
        // pending state before any pin allocation or descriptor verification.
        let outcome = match inner
            .replica
            .apply_publication(&publication, &BTreeSet::new())
        {
            Ok(outcome) => outcome,
            Err(error) => {
                inner.active = None;
                inner.cache.prune_unused_authorizations(context);
                return Err(error.into());
            }
        };
        drop(publication);
        match outcome {
            ApplyOutcome::Applied { revision } => {
                inner.cache.promote_staged_pins(context, id);
                inner.active = None;
                inner.cache.prune_unused_authorizations(context);
                Ok(ReceiveOutcome::Applied { revision })
            }
            ApplyOutcome::Duplicate { revision } => {
                inner.active = None;
                if !Self::usable(inner, context)? {
                    return Err(ReceiverError::NotPresentable);
                }
                Ok(ReceiveOutcome::Duplicate { revision })
            }
            ApplyOutcome::PendingResources { missing, .. } => {
                drop(missing);
                let pins = inner.replica.pending_resources().unwrap();
                let staged = PinOwner::Staged {
                    view_id: context.view_id,
                    publication_id: id,
                };
                let validation = Self::check_fonts(inner.replica.pending_state().unwrap())
                    .and_then(|()| {
                        inner
                            .cache
                            .replace_pins(context, staged, pins)
                            .map_err(Into::into)
                    });
                if let Err(error) = validation {
                    inner.replica.abort_pending();
                    inner.active = None;
                    inner.cache.prune_unused_authorizations(context);
                    return Err(error);
                }
                inner.active.as_mut().unwrap().pending = true;
                self.retry_pending()
            }
        }
    }

    /// Complete exact bytes in one bound envelope. The decoded envelope and
    /// assembly buffers are reserved before allocation. No reserialization is
    /// used for publication identity or integrity.
    pub fn receive_envelope(
        &mut self,
        id: PublicationId,
        bytes: &[u8],
    ) -> Result<ReceiveOutcome, ReceiverError> {
        self.check(self.context)?;
        let outcome = self.receive_envelope_inner(id, bytes);
        if outcome.is_err() {
            let inner = self.inner.as_mut().unwrap();
            if inner.active.is_none() {
                inner.cache.prune_unused_authorizations(self.context);
            }
        }
        outcome
    }
    fn receive_envelope_inner(
        &mut self,
        id: PublicationId,
        bytes: &[u8],
    ) -> Result<ReceiveOutcome, ReceiverError> {
        let limits = self.inner()?.replica.limits();
        let charge = wire::preflight(
            bytes,
            wire::Context::Envelope,
            limits.max_envelope_bytes,
            limits,
        )?;
        let memory = self.budget.reserve(charge)?;
        let envelope = wire::decode_bound_envelope(
            bytes,
            limits,
            &self.context.scope.session_id,
            &self.context.scope.epoch,
            &self.context.view_id,
        )?;
        let payload_context = match proto::MessageKind::try_from(envelope.kind) {
            Ok(proto::MessageKind::Snapshot) => wire::Context::Snapshot,
            Ok(proto::MessageKind::Transaction) => wire::Context::Transaction,
            _ => return Err(WireError::Envelope.into()),
        };
        let (first, second, count) = wire::publication_header(&envelope.payload, payload_context)?;
        let kind = match payload_context {
            wire::Context::Snapshot => PublicationKind::Snapshot { revision: first },
            _ => PublicationKind::Transaction {
                base_revision: first,
                new_revision: second,
            },
        };
        // Automatic chunking respects the configured assembler ceiling.
        let chunk_bytes = self.inner()?.assembler.max_chunk_bytes();
        let chunks = envelope.payload.len().div_ceil(chunk_bytes);
        let begin = PublicationBegin {
            publication_id: id,
            kind,
            total_bytes: envelope.payload.len() as u64,
            chunk_count: u32::try_from(chunks).map_err(|_| PublicationError::Limit)?,
            item_count: u32::try_from(count).map_err(|_| PublicationError::Limit)?,
            digest: publication_digest(&envelope.payload),
        };
        self.begin_publication(self.context, begin.clone())?;
        if self
            .inner()?
            .active
            .as_ref()
            .is_some_and(|active| active.pending)
        {
            drop(envelope);
            drop(memory);
            return self.retry_pending();
        }
        for (index, chunk) in envelope.payload.chunks(chunk_bytes).enumerate() {
            if let Err(error) = self.publication_chunk(self.context, id, index as u32, chunk) {
                self.abort_publication(self.context, id)?;
                return Err(error);
            }
        }
        drop(envelope);
        drop(memory);
        self.commit_publication(self.context, id, PublicationCommit::from(&begin))
    }

    pub fn retry_pending(&mut self) -> Result<ReceiveOutcome, ReceiverError> {
        self.check(self.context)?;
        let context = self.context;
        let inner = self.inner.as_mut().unwrap();
        let active = inner
            .active
            .as_ref()
            .filter(|active| active.pending)
            .ok_or(ModelError::Pending)?;
        let id = active.begin.publication_id;
        let state = inner.replica.pending_state().unwrap();
        if let Err(error) = Self::check_descriptors(&inner.cache, context, state) {
            inner.replica.abort_pending();
            inner.cache.release_pins(
                context,
                PinOwner::Staged {
                    view_id: context.view_id,
                    publication_id: id,
                },
            )?;
            inner.cache.abort_receiving();
            inner.active = None;
            inner.cache.prune_unused_authorizations(context);
            return Err(error);
        }
        let available = inner.cache.available_for_view(context)?;
        let outcome = inner.replica.resume_pending(&available)?;
        drop(available);
        match outcome {
            ApplyOutcome::Applied { revision } => {
                inner.cache.promote_staged_pins(context, id);
                inner.active = None;
                inner.cache.prune_unused_authorizations(context);
                Ok(ReceiveOutcome::Applied { revision })
            }
            ApplyOutcome::PendingResources { revision, missing } => {
                drop(missing);
                let missing = Self::needed(
                    &self.budget,
                    &self.reply_quota,
                    inner,
                    context,
                    inner.replica.pending_resources().unwrap(),
                )?;
                Ok(ReceiveOutcome::NeedResources {
                    publication_id: id,
                    revision,
                    missing,
                })
            }
            ApplyOutcome::Duplicate { .. } => {
                unreachable!("resume only installs a validated pending graph")
            }
        }
    }

    fn check_fonts(state: &State) -> Result<(), ReceiverError> {
        if state.records.values().any(|record| matches!(record, Record::Scene(scene) if matches!(scene.primitive, Primitive::GlyphRun { .. }))) { return Err(ReceiverError::UnsupportedFont); }
        Ok(())
    }
    fn check_descriptors(
        cache: &ResourceCache,
        context: ResourceContext,
        state: &State,
    ) -> Result<(), ReceiverError> {
        let check = |digest, kind| match cache.available_offer(context, digest) {
            Ok(offer) if offer.kind != kind => Err(ReceiverError::ResourceKind),
            Ok(_) | Err(ResourceError::Unavailable) => Ok(()),
            Err(error) => Err(error.into()),
        };
        for record in state.records.values() {
            match record {
                Record::Scene(scene) => match &scene.primitive {
                    Primitive::FillPath { resource, .. }
                    | Primitive::StrokePath { resource, .. } => {
                        check(*resource, ResourceKind::VectorPath)?
                    }
                    Primitive::GlyphRun {
                        font: GlyphFont::Outline(resource),
                        ..
                    } => check(*resource, ResourceKind::GlyphOutline)?,
                    _ => {}
                },
                Record::Session(session) => {
                    if let Some(cursor) = &session.cursor {
                        if let CursorAppearance::Outline(resource) = cursor.appearance {
                            check(resource, ResourceKind::VectorPath)?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn check_offer_kind(
        state: &State,
        digest: ResourceDigest,
        kind: ResourceKind,
    ) -> Result<(), ReceiverError> {
        for record in state.records.values() {
            let expected = match record {
                Record::Scene(scene) => match &scene.primitive {
                    Primitive::FillPath { resource, .. }
                    | Primitive::StrokePath { resource, .. }
                        if *resource == digest =>
                    {
                        Some(ResourceKind::VectorPath)
                    }
                    Primitive::GlyphRun {
                        font: GlyphFont::Outline(resource),
                        ..
                    } if *resource == digest => Some(ResourceKind::GlyphOutline),
                    _ => None,
                },
                Record::Session(session) => match &session.cursor {
                    Some(cursor) if matches!(cursor.appearance, CursorAppearance::Outline(resource) if resource == digest) => {
                        Some(ResourceKind::VectorPath)
                    }
                    _ => None,
                },
                _ => None,
            };
            if expected.is_some_and(|expected| expected != kind) {
                return Err(ReceiverError::ResourceKind);
            }
        }
        Ok(())
    }
    fn is_needed(inner: &Inner, digest: ResourceDigest) -> bool {
        inner.replica.resource_refs().contains(&digest)
            || inner
                .replica
                .pending_resources()
                .is_some_and(|refs| refs.contains(&digest))
    }
    fn needed(
        budget: &MemoryBudget,
        reply_quota: &MemoryBudget,
        inner: &Inner,
        context: ResourceContext,
        refs: &BTreeSet<ResourceDigest>,
    ) -> Result<NeededResources, ReceiverError> {
        let missing = || {
            refs.iter()
                .filter(|digest| inner.cache.read(context, **digest).is_err())
                .copied()
        };
        let count = missing().count();
        let charge = count
            .checked_mul(96)
            .and_then(|n| n.checked_add(if count == 0 { 0 } else { 512 }))
            .ok_or(MemoryError::Overflow)?;
        let memory = budget.reserve(charge)?;
        let reply_memory = reply_quota.reserve(charge)?;
        Ok(NeededResources {
            digests: missing().collect(),
            _memory: memory,
            _reply_quota: reply_memory,
        })
    }
    fn usable(inner: &Inner, context: ResourceContext) -> Result<bool, ReceiverError> {
        if inner.replica.state().revision == 0 {
            return Ok(false);
        }
        Self::check_descriptors(&inner.cache, context, inner.replica.state())?;
        Ok(inner
            .replica
            .resource_refs()
            .iter()
            .all(|digest| inner.cache.read(context, *digest).is_ok()))
    }
    pub fn resources_ready(&self) -> Result<bool, ReceiverError> {
        self.check(self.context)?;
        Self::usable(self.inner()?, self.context)
    }
    pub fn missing_live_resources(&self) -> Result<NeededResources, ReceiverError> {
        self.check(self.context)?;
        let inner = self.inner()?;
        Self::needed(
            &self.budget,
            &self.reply_quota,
            inner,
            self.context,
            inner.replica.resource_refs(),
        )
    }
    pub fn receive_resource_offer(
        &mut self,
        context: ResourceContext,
        offer: ResourceOffer,
    ) -> Result<OfferOutcome, ReceiverError> {
        self.check(context)?;
        let inner = self.inner.as_mut().unwrap();
        if offer.digest == [0; 32] {
            return Err(ReceiverError::InvalidId);
        }
        if !Self::is_needed(inner, offer.digest) {
            return Err(ReceiverError::NotNeeded);
        }
        Self::check_offer_kind(inner.replica.state(), offer.digest, offer.kind)?;
        if let Some(state) = inner.replica.pending_state() {
            Self::check_offer_kind(state, offer.digest, offer.kind)?;
        }
        Ok(inner.cache.receive_offer(context, offer)?)
    }
    pub fn resource_chunk(
        &mut self,
        context: ResourceContext,
        digest: ResourceDigest,
        index: u32,
        bytes: &[u8],
    ) -> Result<ChunkOutcome, ReceiverError> {
        self.check(context)?;
        let inner = self.inner.as_mut().unwrap();
        if !Self::is_needed(inner, digest) {
            return Err(ReceiverError::NotNeeded);
        }
        Ok(inner.cache.receive_chunk(context, digest, index, bytes)?)
    }
    pub fn complete_resource(
        &mut self,
        context: ResourceContext,
        digest: ResourceDigest,
    ) -> Result<Option<ReceiveOutcome>, ReceiverError> {
        self.check(context)?;
        let inner = self.inner.as_mut().unwrap();
        if !Self::is_needed(inner, digest) {
            return Err(ReceiverError::NotNeeded);
        }
        inner.cache.complete(context, digest)?;
        if inner.active.as_ref().is_some_and(|active| active.pending) {
            return self.retry_pending().map(Some);
        }
        Self::check_descriptors(&inner.cache, context, inner.replica.state())?;
        Ok(None)
    }
    pub fn read_resource(&self, digest: ResourceDigest) -> Result<&[u8], ReceiverError> {
        self.check(self.context)?;
        let inner = self.inner()?;
        if !Self::is_needed(inner, digest) {
            return Err(ReceiverError::NotNeeded);
        }
        Ok(inner.cache.read(self.context, digest)?)
    }
    /// Trusted policy revocation closes a view whose retained scene still uses
    /// the resource, rather than leaving a readable unauthorized live scene.
    pub fn revoke_resource_authorization(
        &mut self,
        digest: ResourceDigest,
    ) -> Result<(), ReceiverError> {
        self.check(self.context)?;
        if Self::is_needed(self.inner()?, digest) {
            self.close();
        } else {
            self.inner
                .as_mut()
                .unwrap()
                .cache
                .revoke_resource(self.context.view_id, digest);
        }
        Ok(())
    }
    pub fn evict_resource(
        &mut self,
        digest: ResourceDigest,
    ) -> Result<ResourceEvicted, ReceiverError> {
        self.check(self.context)?;
        let inner = self.inner.as_mut().unwrap();
        let eviction = inner.cache.evict(self.context, digest)?;
        inner.cache.prune_unused_authorizations(self.context);
        Ok(eviction)
    }
    /// Trusted renderer callback; only the current committed revision has a
    /// retained resource closure, so older remembered revisions cannot present.
    pub fn mark_presented(&mut self, revision: u64) -> Result<PresentedAck, ReceiverError> {
        self.check(self.context)?;
        let inner = self.inner.as_mut().unwrap();
        if revision != inner.replica.state().revision || !Self::usable(inner, self.context)? {
            return Err(ReceiverError::NotPresentable);
        }
        inner.replica.mark_presented(revision)?;
        Ok(PresentedAck { revision })
    }
    pub fn abort_publication(
        &mut self,
        context: ResourceContext,
        id: PublicationId,
    ) -> Result<bool, ReceiverError> {
        self.check(context)?;
        let inner = self.inner.as_mut().unwrap();
        if !inner
            .active
            .as_ref()
            .is_some_and(|active| active.begin.publication_id == id)
        {
            return Ok(false);
        }
        inner.assembler.abort(context, id)?;
        inner.replica.abort_pending();
        inner.cache.release_pins(
            context,
            PinOwner::Staged {
                view_id: context.view_id,
                publication_id: id,
            },
        )?;
        inner.cache.abort_receiving();
        inner.active = None;
        inner.cache.prune_unused_authorizations(context);
        Ok(true)
    }
    /// Same binding may reconnect only after the trusted connection layer has
    /// reauthenticated and reauthorized it. Changed policy/epoch needs a new receiver.
    pub fn reconnect(&mut self, context: ResourceContext) -> Result<(), ReceiverError> {
        if self.inner.is_none() {
            return Err(ReceiverError::Closed);
        }
        if context != self.context {
            return Err(ReceiverError::Unauthorized);
        }
        self.connected = true;
        Ok(())
    }
    pub fn disconnect(&mut self) {
        if let Some(inner) = self.inner.as_mut() {
            inner
                .assembler
                .abort_all(self.context)
                .expect("private authorized assembler");
            inner.replica.abort_pending();
            inner.replica.reset_presentation();
            if let Some(active) = inner.active.take() {
                inner
                    .cache
                    .release_pins(
                        self.context,
                        PinOwner::Staged {
                            view_id: self.context.view_id,
                            publication_id: active.begin.publication_id,
                        },
                    )
                    .expect("private authorized cache");
            }
            inner.cache.abort_receiving();
            inner.cache.prune_unused_authorizations(self.context);
        }
        self.connected = false;
    }
    /// Trusted cache restart: pins and state survive, presentation readiness
    /// clears, and peer resource offers must safely refill the actual closure.
    pub fn resource_cache_lost(&mut self) -> Result<(), ReceiverError> {
        self.check(self.context)?;
        let inner = self.inner.as_mut().unwrap();
        inner.cache.lose_entries();
        inner.cache.prune_unused_authorizations(self.context);
        inner.replica.reset_presentation();
        Ok(())
    }
    /// Irreversible disposal, including staged bytes, decoded state, history,
    /// authorization, verified bodies, transfers and every pin owner.
    pub fn close(&mut self) {
        self.inner = None;
        self.connected = false;
    }
    pub fn revoke(&mut self) {
        self.close();
    }
}
