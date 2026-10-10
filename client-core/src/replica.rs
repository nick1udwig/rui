//! Platform-independent authenticated replica/session transitions. Socket admission,
//! deadlines and presentation opportunities belong to adapters; replies are admitted
//! synchronously into their bounded charged queues before an event can escape.
use crate::{command_queue::CommandQueue, resource_requests::ResourceRequests};
use prost::Message;
use rui_protocol_core::wire::proto as commands;
use rui_protocol_core::{
    commands::{CommandFamily, Id},
    receiver::{ReceiveOutcome, ViewReceiver},
    resources::{MemoryReservation, ResourceContext},
    session::ReceiverLimits,
};
use rui_wire::{codec, proto as p, WireError as TransportError};
#[derive(Debug)]
pub enum ClientEvent {
    ViewOpened(OpenedView),
    Applied { revision: u64 },
    ResourcesNeeded { revision: u64 },
    ResourceAvailable,
    ControlGranted(p::ControlGranted),
    ControlRevoked(p::ControlRevoked),
    CommandResult(p::CommandResult),
    ResumeAccepted(p::ResumeAccepted),
    ResetRequired(p::ResetRequired),
    Pong(u64),
    Error(String),
}
/// Immutable authenticated view metadata. Its conservative decoded allocation
/// reservation survives every escaped handle and is released when it drops.
pub struct OpenedView {
    wire: p::ViewOpened,
    _memory: MemoryReservation,
}
impl std::ops::Deref for OpenedView {
    type Target = p::ViewOpened;
    fn deref(&self) -> &Self::Target {
        &self.wire
    }
}
impl std::fmt::Debug for OpenedView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenedView")
            .field("canvas_width", &self.wire.canvas_width)
            .field("canvas_height", &self.wire.canvas_height)
            .field("coverage", &self.wire.coverage)
            .field("input_device_count", &self.wire.input_devices.len())
            .finish()
    }
}

/// Reservation before payload construction is part of the platform admission contract.
/// Implementations keep accepted bytes/reservations until their sink owns the frame.
pub trait Replies {
    type Reservation;
    fn reserve(
        &self,
        kind: p::Kind,
        payload_len: usize,
        binding: Option<(Id, Id, Id)>,
    ) -> Result<Self::Reservation, TransportError>;
    fn enqueue_reserved<T: Message>(
        &mut self,
        kind: p::Kind,
        binding: Option<(Id, Id, Id)>,
        reservation: Self::Reservation,
        make: impl FnOnce() -> T,
    ) -> Result<(), TransportError>;
    fn enqueue_with<T: Message>(
        &mut self,
        kind: p::Kind,
        payload_len: usize,
        binding: Option<(Id, Id, Id)>,
        make: impl FnOnce() -> T,
    ) -> Result<(), TransportError> {
        let r = self.reserve(kind, payload_len, binding)?;
        self.enqueue_reserved(kind, binding, r, make)
    }
    fn enqueue<T: Message>(
        &mut self,
        kind: p::Kind,
        payload: &T,
        binding: Option<(Id, Id, Id)>,
    ) -> Result<(), TransportError>;
}

struct HeldLease {
    wire: p::ControlGranted,
    _memory: MemoryReservation,
}

pub struct Replica {
    context: ResourceContext,
    connected: bool,
    limits: ReceiverLimits,
    semantic_only: bool,
    receiver: ViewReceiver,
    resources: ResourceRequests,
    commands: CommandQueue,
    lease: Option<HeldLease>,
    ready: Option<u64>,
    profiles: Vec<String>,
    permissions: Option<p::Permissions>,
    capabilities_memory: Option<MemoryReservation>,
    awaiting_resume: Option<u64>,
    resume_applied: Option<u64>,
    resume_needed: Option<u64>,
}
impl Replica {
    pub fn new(
        context: ResourceContext,
        limits: ReceiverLimits,
        semantic_only: bool,
        receiver: ViewReceiver,
        awaiting_resume: Option<u64>,
    ) -> Result<Self, TransportError> {
        let commands = CommandQueue::new(
            context.scope.epoch,
            receiver.memory_budget(),
            limits.envelope_bytes.saturating_sub(128),
        )
        .map_err(|_| TransportError::Limit)?;
        Ok(Self::with_commands(
            context,
            limits,
            semantic_only,
            receiver,
            awaiting_resume,
            commands,
        ))
    }
    /// Assemble previously reserved owners without a fallible transfer boundary.
    pub fn with_commands(
        context: ResourceContext,
        limits: ReceiverLimits,
        semantic_only: bool,
        receiver: ViewReceiver,
        awaiting_resume: Option<u64>,
        commands: CommandQueue,
    ) -> Self {
        Self {
            context,
            connected: true,
            limits,
            semantic_only,
            receiver,
            awaiting_resume,
            resources: ResourceRequests::default(),
            commands,
            lease: None,
            ready: None,
            profiles: Vec::new(),
            permissions: None,
            capabilities_memory: None,
            resume_applied: None,
            resume_needed: None,
        }
    }
    pub fn configure(
        &mut self,
        profiles: &[String],
        permissions: &p::Permissions,
    ) -> Result<(), TransportError> {
        let capabilities =
            reserve_capabilities(profiles, permissions, self.receiver.memory_budget())?;
        self.install_capabilities(capabilities);
        Ok(())
    }
    pub fn install_capabilities(&mut self, capabilities: NegotiatedCapabilities) {
        self.profiles = capabilities.profiles;
        self.permissions = Some(capabilities.permissions);
        self.capabilities_memory = Some(capabilities.memory);
    }
    pub fn profiles(&self) -> &[String] {
        &self.profiles
    }
    pub fn permissions(&self) -> Option<&p::Permissions> {
        self.permissions.as_ref()
    }
    pub fn ready(&self) -> Option<u64> {
        self.ready
    }
    pub fn clear_readiness(&mut self) {
        self.ready = None;
    }
    pub fn commands(&self) -> &CommandQueue {
        &self.commands
    }
    pub fn commands_mut(&mut self) -> &mut CommandQueue {
        &mut self.commands
    }
    /// Rebind retained exact commands only after an authenticated scope change.
    /// Failed memory reservations leave the old actor and commands intact.
    pub fn inherit_commands(&mut self, old: &mut Self) -> Result<(), TransportError> {
        old.commands
            .rebind(
                self.context.scope.epoch,
                self.receiver.memory_budget(),
                self.limits.envelope_bytes.saturating_sub(128),
            )
            .map_err(|_| TransportError::Limit)?;
        std::mem::swap(&mut self.commands, &mut old.commands);
        Ok(())
    }
    pub fn lease(&self) -> Option<&p::ControlGranted> {
        self.lease.as_ref().map(|g| &g.wire)
    }
    pub fn release_lease(&mut self) -> Option<p::ControlGranted> {
        self.lease.take().map(|g| g.wire)
    }
    pub fn retain_command(
        &mut self,
        family: CommandFamily,
        bytes: &[u8],
    ) -> Result<(), TransportError> {
        if !self.connected {
            return Err(TransportError::Closed);
        }
        if bytes.len() > self.limits.envelope_bytes.saturating_sub(128) {
            return Err(TransportError::Limit);
        }
        let _decode = self
            .receiver
            .memory_budget()
            .reserve(
                bytes
                    .len()
                    .checked_mul(4)
                    .and_then(|n| n.checked_add(8192))
                    .ok_or(TransportError::Limit)?,
            )
            .map_err(|_| TransportError::Limit)?;
        // Only locally supplied bounded command bytes; the host still validates
        // the full packet and authoritative effect. Invalid probes use send_packet.
        let sequence = match family {
            CommandFamily::Semantic => {
                commands::SemanticCommand::decode(bytes)
                    .map_err(|_| TransportError::Protocol)?
                    .command_seq
            }
            CommandFamily::RawInput => {
                commands::RawInputBatch::decode(bytes)
                    .map_err(|_| TransportError::Protocol)?
                    .header
                    .ok_or(TransportError::Protocol)?
                    .command_seq
            }
            CommandFamily::TextComposition => {
                commands::TextComposition::decode(bytes)
                    .map_err(|_| TransportError::Protocol)?
                    .header
                    .ok_or(TransportError::Protocol)?
                    .command_seq
            }
            CommandFamily::Resize => {
                commands::ResizeRequest::decode(bytes)
                    .map_err(|_| TransportError::Protocol)?
                    .header
                    .ok_or(TransportError::Protocol)?
                    .command_seq
            }
        };
        self.commands
            .retain_exact(sequence, family, bytes)
            .map_err(|_| TransportError::Limit)
    }
    pub fn is_connected(&self) -> bool {
        self.connected
    }
    pub fn receiver(&self) -> &ViewReceiver {
        &self.receiver
    }
    pub fn into_parts(self) -> (ViewReceiver, CommandQueue) {
        (self.receiver, self.commands)
    }
    pub fn reconnect(&mut self, awaiting_resume: Option<u64>) -> Result<(), TransportError> {
        self.receiver
            .reconnect(self.context)
            .map_err(|_| TransportError::Receiver)?;
        self.connected = true;
        self.resources.clear();
        self.lease = None;
        self.ready = None;
        self.awaiting_resume = awaiting_resume;
        self.resume_applied = None;
        self.resume_needed = None;
        Ok(())
    }
    pub fn disconnect(&mut self) {
        self.connected = false;
        self.awaiting_resume = None;
        self.receiver.disconnect();
        self.lease = None;
        self.ready = None;
        self.resources.clear();
        self.resume_applied = None;
        self.resume_needed = None;
    }
    pub fn close(&mut self) {
        self.disconnect();
        self.receiver.close();
        self.commands.clear();
        self.profiles = Vec::new();
        self.permissions = None;
        self.capabilities_memory = None;
    }
    fn binding(&self) -> Option<(Id, Id, Id)> {
        Some((
            self.context.scope.session_id,
            self.context.scope.epoch,
            self.context.view_id,
        ))
    }
    pub fn presented(
        &mut self,
        revision: u64,
        replies: &mut impl Replies,
    ) -> Result<(), TransportError> {
        if self.semantic_only {
            return Err(TransportError::Authority);
        }
        self.receiver
            .mark_presented(revision)
            .map_err(|_| TransportError::Receiver)?;
        replies.enqueue(
            p::Kind::PresentedAck,
            &p::RevisionAck { revision },
            self.binding(),
        )?;
        self.ready = Some(revision);
        Ok(())
    }
    pub fn semantic_ready(
        &mut self,
        revision: u64,
        replies: &mut impl Replies,
    ) -> Result<(), TransportError> {
        if !self.semantic_only
            || self
                .receiver
                .state()
                .map_err(|_| TransportError::Receiver)?
                .revision
                != revision
            || !self
                .receiver
                .resources_ready()
                .map_err(|_| TransportError::Receiver)?
        {
            return Err(TransportError::Authority);
        }
        replies.enqueue(
            p::Kind::SemanticReady,
            &p::RevisionAck { revision },
            self.binding(),
        )?;
        self.ready = Some(revision);
        Ok(())
    }
    fn outcome(
        &mut self,
        outcome: ReceiveOutcome,
        replies: &mut impl Replies,
    ) -> Result<ClientEvent, TransportError> {
        match outcome {
            ReceiveOutcome::Applied { revision } | ReceiveOutcome::Duplicate { revision } => {
                self.resources.clear();
                self.ready = None;
                replies.enqueue(
                    p::Kind::AppliedAck,
                    &p::RevisionAck { revision },
                    self.binding(),
                )?;
                Ok(ClientEvent::Applied { revision })
            }
            ReceiveOutcome::NeedResources {
                revision, missing, ..
            } => {
                self.ready = None;
                self.resources.begin(missing, false)?;
                self.queue_resource_request(replies)?;
                Ok(ClientEvent::ResourcesNeeded { revision })
            }
        }
    }
    pub fn queue_resource_request(
        &mut self,
        replies: &mut impl Replies,
    ) -> Result<(), TransportError> {
        let Some(page) = self.resources.page(self.limits.envelope_bytes)? else {
            return Ok(());
        };
        let binding = self.binding();
        if page.announce_evictions {
            replies.enqueue_with(
                p::Kind::ResourceEvicted,
                ResourceRequests::payload_len(page),
                binding,
                || self.resources.payload(page),
            )?;
        }
        replies.enqueue_with(
            p::Kind::NeedResources,
            ResourceRequests::payload_len(page),
            binding,
            || self.resources.payload(page),
        )?;
        self.resources.queued(page);
        Ok(())
    }
    /// Ordinary cache pressure releases one retired, unpinned entry. Current
    /// graph and candidate dependencies cannot be evicted by this operation.
    /// Cancellation retains the queued notification for the next flush.
    pub fn evict_unpinned_resource(
        &mut self,
        replies: &mut impl Replies,
    ) -> Result<Option<[u8; 32]>, TransportError> {
        let binding = self.binding();
        let reservation = replies.reserve(p::Kind::ResourceEvicted, 34, binding)?;
        let Some(evicted) = self
            .receiver
            .evict_one_unpinned_resource()
            .map_err(|_| TransportError::Receiver)?
        else {
            return Ok(None);
        };
        let digest = evicted.digest;
        replies.enqueue_reserved(p::Kind::ResourceEvicted, binding, reservation, || {
            p::ResourceIds {
                digests: vec![digest.to_vec()],
            }
        })?;
        Ok(Some(digest))
    }
    /// Trusted local cache-loss signal. It pauses presentation immediately;
    /// resource recovery is requested in bounded pages and APPLIED is re-sent
    /// only after the live closure is verified. A pending candidate must first
    /// abort/reconnect rather than mixing its receiving transfer with cache loss.
    pub fn resource_cache_lost(
        &mut self,
        replies: &mut impl Replies,
    ) -> Result<(), TransportError> {
        if self.receiver.has_pending_publication() {
            return Err(TransportError::Receiver);
        }
        // There is no cache-loss transition for a resource-less graph. In
        // particular, do not clear local presentation while the host still has
        // its valid readiness and no missing-digest marker can be sent.
        if self
            .receiver
            .cache_bytes()
            .map_err(|_| TransportError::Receiver)?
            == 0
            && self
                .receiver
                .resources_ready()
                .map_err(|_| TransportError::Receiver)?
        {
            return Ok(());
        }
        self.ready = None;
        self.receiver
            .resource_cache_lost()
            .map_err(|_| TransportError::Receiver)?;
        let missing = self
            .receiver
            .missing_live_resources()
            .map_err(|_| TransportError::Receiver)?;
        if missing.is_empty() {
            let revision = self
                .receiver
                .state()
                .map_err(|_| TransportError::Receiver)?
                .revision;
            replies.enqueue(
                p::Kind::AppliedAck,
                &p::RevisionAck { revision },
                self.binding(),
            )?;
        } else {
            self.resources.begin(missing, true)?;
            self.queue_resource_request(replies)?;
        }
        Ok(())
    }
    /// Deferred events are consumed synchronously before waiting for another packet.
    pub fn poll(
        &mut self,
        replies: &mut impl Replies,
    ) -> Result<Option<ClientEvent>, TransportError> {
        if let Some(revision) = self.resume_needed.take() {
            return Ok(Some(ClientEvent::ResourcesNeeded { revision }));
        }
        if let Some(revision) = self.resume_applied.take() {
            return self
                .outcome(ReceiveOutcome::Applied { revision }, replies)
                .map(Some);
        }
        Ok(None)
    }
    /// Malformed peer input disconnects staging/readiness/authority in both
    /// adapters while preserving committed state and exact pending commands.
    pub fn receive(
        &mut self,
        raw: &[u8],
        replies: &mut impl Replies,
    ) -> Result<Option<ClientEvent>, TransportError> {
        if !self.connected {
            return Err(TransportError::Closed);
        }
        let result = self.receive_inner(raw, replies);
        if result.is_err() {
            self.disconnect();
        }
        result
    }
    fn receive_inner(
        &mut self,
        raw: &[u8],
        replies: &mut impl Replies,
    ) -> Result<Option<ClientEvent>, TransportError> {
        let _decode = self
            .receiver
            .memory_budget()
            .reserve(
                raw.len()
                    .checked_mul(4)
                    .and_then(|n| n.checked_add(8192))
                    .ok_or(TransportError::Limit)?,
            )
            .map_err(|_| TransportError::Limit)?;
        let packet = codec::packet(raw, self.limits.envelope_bytes, self.binding())?;
        let kind = p::Kind::try_from(packet.kind).map_err(|_| TransportError::Protocol)?;
        let ceiling = self.limits.envelope_bytes;
        let context = self.context;
        match kind {
            p::Kind::ViewOpened => {
                let memory = self
                    .receiver
                    .memory_budget()
                    .reserve(
                        packet
                            .payload
                            .len()
                            .checked_mul(4)
                            .and_then(|n| n.checked_add(8192))
                            .ok_or(TransportError::Limit)?,
                    )
                    .map_err(|_| TransportError::Limit)?;
                let wire = codec::view_opened(&packet.payload, ceiling)?;
                if codec::id(&wire.view_id)? != self.context.view_id {
                    return Err(TransportError::Binding);
                }
                return Ok(Some(ClientEvent::ViewOpened(OpenedView {
                    wire,
                    _memory: memory,
                })));
            }
            p::Kind::SnapshotBegin | p::Kind::TransactionBegin => {
                let wire: p::PublicationBegin = codec::decode(&packet.payload, ceiling)?;
                let begin = codec::publication_begin(kind, wire)?;
                self.receiver
                    .begin_publication(context, begin)
                    .map_err(|_| TransportError::Receiver)?;
            }
            p::Kind::PublicationChunk => {
                let chunk: p::PublicationChunk = codec::decode(&packet.payload, ceiling)?;
                if chunk.data.len() > self.limits.chunk_bytes {
                    return Err(TransportError::Limit);
                }
                self.receiver
                    .publication_chunk(
                        context,
                        codec::id(&chunk.publication_id)?,
                        chunk.index,
                        &chunk.data,
                    )
                    .map_err(|_| TransportError::Receiver)?;
            }
            p::Kind::PublicationCommit => {
                let commit: p::PublicationCommit = codec::decode(&packet.payload, ceiling)?;
                let id = codec::id(&commit.publication_id)?;
                let _timing = rui_protocol_core::timing::span(
                    rui_protocol_core::timing::Stage::ReplicaApplication,
                    self.receiver.state().map(|s| s.revision).unwrap_or(0),
                    1,
                );
                let outcome = self
                    .receiver
                    .commit_authenticated_publication(
                        context,
                        id,
                        codec::publication_commit(commit)?,
                    )
                    .map_err(|_| TransportError::Receiver)?;
                return self.outcome(outcome, replies).map(Some);
            }
            p::Kind::AbortPublication => {
                let abort: p::AbortPublication = codec::decode(&packet.payload, ceiling)?;
                self.receiver
                    .abort_publication(context, codec::id(&abort.publication_id)?)
                    .map_err(|_| TransportError::Receiver)?;
            }
            p::Kind::ResourceOffer => {
                let offer = codec::resource_offer(codec::decode(&packet.payload, ceiling)?)?;
                if !self.resources.contains(&offer.digest) {
                    return Err(TransportError::Authentication);
                }
                self.receiver
                    .authorize_resource(offer.digest)
                    .map_err(|_| TransportError::Receiver)?;
                self.receiver
                    .receive_resource_offer(context, offer)
                    .map_err(|_| TransportError::Receiver)?;
            }
            p::Kind::ResourceChunk => {
                let chunk: p::ResourceChunk = codec::decode(&packet.payload, ceiling)?;
                if chunk.data.len() > self.limits.chunk_bytes {
                    return Err(TransportError::Limit);
                }
                self.receiver
                    .resource_chunk(
                        context,
                        codec::digest(&chunk.digest)?,
                        chunk.index,
                        &chunk.data,
                    )
                    .map_err(|_| TransportError::Receiver)?;
            }
            p::Kind::ResourceComplete => {
                let complete: p::ResourceComplete = codec::decode(&packet.payload, ceiling)?;
                let digest = codec::digest(&complete.digest)?;
                let outcome = self
                    .receiver
                    .complete_resource(context, digest)
                    .map_err(|_| TransportError::Receiver)?;
                replies.enqueue_with(p::Kind::ResourceAvailable, 34, self.binding(), || {
                    p::ResourceIds {
                        digests: vec![digest.to_vec()],
                    }
                })?;
                if let Some(outcome) = outcome {
                    // The original, charged request set/cursor already
                    // covers the whole candidate closure. A partial retry
                    // reports a smaller missing set, not a new publication
                    // or a reason to request completed resources again.
                    if matches!(outcome, ReceiveOutcome::NeedResources { .. }) {
                        return Ok(Some(ClientEvent::ResourceAvailable));
                    }
                    return self.outcome(outcome, replies).map(Some);
                }
                if self.resources.active()
                    && !self.receiver.has_pending_publication()
                    && self
                        .receiver
                        .resources_ready()
                        .map_err(|_| TransportError::Receiver)?
                {
                    let revision = self
                        .receiver
                        .state()
                        .map_err(|_| TransportError::Receiver)?
                        .revision;
                    return self
                        .outcome(ReceiveOutcome::Applied { revision }, replies)
                        .map(Some);
                }
                return Ok(Some(ClientEvent::ResourceAvailable));
            }
            p::Kind::ControlGranted => {
                let grant: p::ControlGranted = codec::decode(&packet.payload, ceiling)?;
                codec::id(&grant.request_id)?;
                codec::id(&grant.lease_id)?;
                if grant.generation == 0 {
                    return Err(TransportError::Authority);
                }
                let memory = self
                    .receiver
                    .memory_budget()
                    .reserve(
                        packet
                            .payload
                            .len()
                            .checked_mul(4)
                            .and_then(|n| n.checked_add(8192))
                            .ok_or(TransportError::Limit)?,
                    )
                    .map_err(|_| TransportError::Limit)?;
                self.lease = Some(HeldLease {
                    wire: grant.clone(),
                    _memory: memory,
                });
                return Ok(Some(ClientEvent::ControlGranted(grant)));
            }
            p::Kind::ControlRevoked => {
                let notice: p::ControlRevoked = codec::decode(&packet.payload, ceiling)?;
                if self.lease.as_ref().is_some_and(|g| {
                    g.wire.lease_id == notice.lease_id && g.wire.generation == notice.generation
                }) {
                    self.lease = None;
                }
                return Ok(Some(ClientEvent::ControlRevoked(notice)));
            }
            p::Kind::CommandResult => {
                let result: p::CommandResult = codec::decode(&packet.payload, ceiling)?;
                self.commands.result(result.command_seq, &result.status);
                return Ok(Some(ClientEvent::CommandResult(result)));
            }
            p::Kind::ResumeAccepted => {
                let resumed: p::ResumeAccepted = codec::decode(&packet.payload, ceiling)?;
                let revision = self
                    .receiver
                    .state()
                    .map_err(|_| TransportError::Receiver)?
                    .revision;
                if self.awaiting_resume.take() != Some(resumed.from_revision)
                    || revision != resumed.from_revision
                    || resumed.through_revision < revision
                {
                    return Err(TransportError::Protocol);
                }
                if resumed.through_revision == revision {
                    let missing = self
                        .receiver
                        .missing_live_resources()
                        .map_err(|_| TransportError::Receiver)?;
                    if missing.is_empty() {
                        self.resume_applied = Some(revision);
                    } else {
                        self.resources.begin(missing, true)?;
                        self.resume_needed = Some(revision);
                        self.queue_resource_request(replies)?;
                    }
                }
                return Ok(Some(ClientEvent::ResumeAccepted(resumed)));
            }
            p::Kind::ResetRequired => {
                self.ready = None;
                if self.awaiting_resume.take().is_none() {
                    return Err(TransportError::Protocol);
                }
                // A higher checkpoint preserves this retained reducer's
                // retirement and monotonic clocks under the same view.
                return Ok(Some(ClientEvent::ResetRequired(codec::decode(
                    &packet.payload,
                    ceiling,
                )?)));
            }
            p::Kind::Error => {
                self.lease = None;
                let error: p::Error = codec::decode(&packet.payload, ceiling)?;
                return Ok(Some(ClientEvent::Error(error.code)));
            }
            p::Kind::Pong => {
                let pong: p::Ping = codec::decode(&packet.payload, ceiling)?;
                return Ok(Some(ClientEvent::Pong(pong.nonce)));
            }
            _ => return Err(TransportError::Protocol),
        }
        Ok(None)
    }
}

/// Exact envelope length, without allocating a temporary payload/Packet.
pub fn message_size(
    kind: p::Kind,
    payload_len: usize,
    binding: Option<(Id, Id, Id)>,
) -> Result<usize, TransportError> {
    fn varint_len(mut n: usize) -> usize {
        let mut len = 1;
        while n >= 128 {
            n >>= 7;
            len += 1;
        }
        len
    }
    // Exactly codec::wrap's fields, without allocating a temporary Packet.
    let bytes = 2usize
        .checked_add(1 + varint_len(kind as usize))
        .and_then(|n| n.checked_add(if binding.is_some() { 54 } else { 0 }))
        .and_then(|n| {
            n.checked_add(if payload_len == 0 {
                0
            } else {
                1 + varint_len(payload_len)
            })
        })
        .and_then(|n| n.checked_add(payload_len))
        .ok_or(TransportError::Limit)?;
    Ok(bytes)
}

/// One negotiation validator for native TLS and the portable WASM adapter.
/// Authentication/Origin/certificate checks remain platform responsibilities.
pub struct Negotiated {
    pub server: p::ServerHello,
    pub limits: ReceiverLimits,
    pub context: ResourceContext,
}
pub fn negotiate_server(
    raw: &[u8],
    hello: &rui_protocol_core::session::ClientHello,
    exact_envelope: bool,
) -> Result<Negotiated, TransportError> {
    let ceiling = hello.receiver_limits.envelope_bytes;
    let initial: p::Packet = codec::decode(raw, ceiling)?;
    if initial.kind == p::Kind::Error as i32 {
        let error: p::Error = codec::decode(&initial.payload, ceiling)?;
        return Err(if error.code == "PRODUCER_CONNECTION_BUSY" {
            TransportError::ProducerBusy
        } else {
            TransportError::Authentication
        });
    }
    let binding = (
        codec::id(&initial.session_id)?,
        codec::id(&initial.epoch)?,
        codec::id(&initial.view_id)?,
    );
    let packet = codec::packet(raw, ceiling, Some(binding))?;
    if packet.kind != p::Kind::ServerHello as i32 {
        return Err(TransportError::Protocol);
    }
    let mut server: p::ServerHello = codec::decode(&packet.payload, ceiling)?;
    let limits = codec::limits(
        server
            .negotiated_limits
            .take()
            .ok_or(TransportError::Protocol)?,
    )?;
    if (exact_envelope && limits.envelope_bytes != ceiling)
        || server.selected_version != 1
        || hello
            .receiver_limits
            .negotiate(&limits)
            .map_err(|_| TransportError::Limit)?
            != limits
    {
        return Err(TransportError::Limit);
    }
    if !hello.supported_versions.contains(&server.selected_version)
        || server
            .selected_profiles
            .iter()
            .any(|p| !hello.supported_profiles.contains(p))
        || hello
            .required_profiles
            .iter()
            .any(|p| !server.selected_profiles.contains(p))
    {
        return Err(TransportError::Protocol);
    }
    rui_protocol_core::wire::required_features(&server.selected_profiles)
        .map_err(|_| TransportError::Protocol)?;
    if !server
        .permissions
        .as_ref()
        .ok_or(TransportError::Protocol)?
        .can_view
    {
        return Err(TransportError::Authority);
    }
    let context = ResourceContext {
        scope: rui_protocol_core::resources::ResourceScope {
            session_id: binding.0,
            epoch: binding.1,
            principal_id: codec::id(&server.principal_id)?,
        },
        view_id: binding.2,
    };
    Ok(Negotiated {
        server,
        limits,
        context,
    })
}

/// Charged negotiated metadata, constructed before transferring a retained owner.
pub struct NegotiatedCapabilities {
    profiles: Vec<String>,
    permissions: p::Permissions,
    memory: MemoryReservation,
}
pub fn reserve_capabilities(
    profiles: &[String],
    permissions: &p::Permissions,
    budget: rui_protocol_core::resources::MemoryBudget,
) -> Result<NegotiatedCapabilities, TransportError> {
    let strings = profiles
        .iter()
        .try_fold(0usize, |n, s| {
            n.checked_add(s.len())
                .and_then(|n| n.checked_add(std::mem::size_of::<String>()))
        })
        .ok_or(TransportError::Limit)?;
    let charge = strings
        .checked_add(permissions.encoded_len())
        .and_then(|n| n.checked_mul(4))
        .and_then(|n| n.checked_add(8192))
        .ok_or(TransportError::Limit)?;
    let memory = budget.reserve(charge).map_err(|_| TransportError::Limit)?;
    Ok(NegotiatedCapabilities {
        profiles: profiles.to_vec(),
        permissions: permissions.clone(),
        memory,
    })
}
