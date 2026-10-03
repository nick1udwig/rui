use crate::{codec, pairing::Credential, proto as p, tls, websocket, TransportError};
use futures_util::SinkExt;
use prost::Message;
use rui_protocol_core::commands::{CommandFamily, Id};
use rui_protocol_core::receiver::{NeededResources, ReceiveOutcome, ReceiverConfig, ViewReceiver};
use rui_protocol_core::resources::{
    MemoryBudget, MemoryReservation, ResourceContext, ResourceScope,
};
use rui_protocol_core::session::{ClientHello, ReceiverLimits, ResumeRequest};
use std::{collections::VecDeque, net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    time::timeout,
};
use tokio_tungstenite::{
    tungstenite::{Bytes, Message as WsMessage},
    WebSocketStream,
};
use zeroize::Zeroize;

#[derive(Debug)]
pub enum ClientEvent {
    ViewOpened(p::ViewOpened),
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

type ClientSocket =
    WebSocketStream<websocket::ResponseGuard<tokio_rustls::client::TlsStream<TcpStream>>>;
/// Committed graph/cache owned locally after socket loss. Staging, queued replies
/// and presentation are discarded; reauthentication alone cannot restore input.
pub struct RetainedClientView {
    context: ResourceContext,
    client_instance: Id,
    limits: ReceiverLimits,
    profiles: Vec<String>,
    receiver: Option<ViewReceiver>,
}
impl RetainedClientView {
    pub fn receiver(&self) -> Option<&ViewReceiver> {
        self.receiver.as_ref()
    }
    pub fn lose_resource_cache(&mut self) -> Result<(), TransportError> {
        let receiver = self.receiver.as_mut().ok_or(TransportError::Receiver)?;
        receiver
            .reconnect(self.context)
            .map_err(|_| TransportError::Receiver)?;
        let result = receiver
            .resource_cache_lost()
            .map_err(|_| TransportError::Receiver);
        receiver.disconnect();
        result
    }
    /// Reauthenticate this retained producer; call `open_view` on success to
    /// receive the resume/checkpoint. Failed or cancelled attempts retain this
    /// owner. Only an authenticated producer-busy response retries (up to 2s).
    pub async fn reconnect(
        &mut self,
        address: SocketAddr,
        server_name: String,
        pinned_certificate: Vec<u8>,
        credential: &Credential,
        mut hello: ClientHello,
    ) -> Result<NetworkClient, TransportError> {
        let receiver = self.receiver.as_ref().ok_or(TransportError::Receiver)?;
        if hello.resume.is_some() || hello.client_instance_id != self.client_instance {
            return Err(TransportError::Protocol);
        }
        let fresh = hello
            .receiver_limits
            .negotiate(&self.limits)
            .map_err(|_| TransportError::Limit)?
            != self.limits
            || self
                .profiles
                .iter()
                .any(|profile| !hello.supported_profiles.contains(profile))
            || hello
                .required_profiles
                .iter()
                .any(|profile| !self.profiles.contains(profile));
        if !fresh {
            hello.resume = Some(ResumeRequest {
                epoch: self.context.scope.epoch,
                view_id: self.context.view_id,
                acknowledged_revision: receiver
                    .state()
                    .map_err(|_| TransportError::Receiver)?
                    .revision,
            });
        }
        NetworkClient::connect_retained(
            address,
            server_name,
            pinned_certificate,
            credential,
            hello,
            Some(self),
            fresh,
        )
        .await
    }
}
pub struct NetworkClient {
    socket: ClientSocket,
    client_instance: Id,
    pub session_id: Id,
    pub epoch: Id,
    pub view_id: Id,
    pub limits: ReceiverLimits,
    pub profiles: Vec<String>,
    context: ResourceContext,
    receiver: ViewReceiver,
    needed: Option<NeededResources>,
    needed_requested: usize,
    announce_evictions: bool,
    deadline: Duration,
    pending: PendingReplies,
    awaiting_resume: Option<u64>,
    resume_applied: Option<u64>,
    resume_needed: Option<u64>,
}

struct PendingReply {
    encoded: Bytes,
    submitted: bool,
    _memory: MemoryReservation,
}
struct PendingReplies {
    entries: VecDeque<PendingReply>,
    bytes: usize,
    envelope_limit: usize,
    queue_limit: usize,
    budget: MemoryBudget,
    // Retained for the socket lifetime: a cancelled flush can leave a frame in
    // the maintained sink, and its allocation may remain after a later flush.
    _writer_memory: MemoryReservation,
}
impl PendingReplies {
    fn new(budget: MemoryBudget, limits: &ReceiverLimits) -> Result<Self, TransportError> {
        let writer_bytes = limits
            .envelope_bytes
            .checked_mul(4)
            .and_then(|n| n.checked_add(64 * 1024))
            .ok_or(TransportError::Limit)?;
        let writer_memory = budget
            .reserve(writer_bytes)
            .map_err(|_| TransportError::Limit)?;
        Ok(Self {
            entries: VecDeque::new(),
            bytes: 0,
            envelope_limit: limits.envelope_bytes,
            queue_limit: limits.outbound_queue_bytes,
            budget,
            _writer_memory: writer_memory,
        })
    }
    fn reserve(&self, bytes: usize) -> Result<MemoryReservation, TransportError> {
        if bytes > self.envelope_limit
            || self.entries.len() >= 8
            || self
                .bytes
                .checked_add(bytes)
                .is_none_or(|n| n > self.queue_limit)
        {
            return Err(TransportError::Limit);
        }
        // Covers sparse VecDeque storage, transient typed payload/Packet and
        // encoding allocations. Sink framing has its separate lifetime charge.
        let charge = bytes
            .checked_mul(4)
            .and_then(|n| n.checked_add(4096))
            .ok_or(TransportError::Limit)?;
        self.budget
            .reserve(charge)
            .map_err(|_| TransportError::Limit)
    }
    fn enqueue_with<T: Message>(
        &mut self,
        kind: p::Kind,
        payload_len: usize,
        binding: Option<(Id, Id, Id)>,
        make: impl FnOnce() -> T,
    ) -> Result<(), TransportError> {
        self.enqueue_reserved(kind, payload_len, binding, || {
            codec::wrap(kind, &make(), binding)
        })
    }
    fn enqueue<T: Message>(
        &mut self,
        kind: p::Kind,
        payload: &T,
        binding: Option<(Id, Id, Id)>,
    ) -> Result<(), TransportError> {
        self.enqueue_reserved(kind, payload.encoded_len(), binding, || {
            codec::wrap(kind, payload, binding)
        })
    }
    fn enqueue_reserved(
        &mut self,
        kind: p::Kind,
        payload_len: usize,
        binding: Option<(Id, Id, Id)>,
        make: impl FnOnce() -> p::Packet,
    ) -> Result<(), TransportError> {
        let (memory, bytes) = self.reserve_message(kind, payload_len, binding)?;
        self.push(make(), memory, bytes)
    }
    fn reserve_message(
        &self,
        kind: p::Kind,
        payload_len: usize,
        binding: Option<(Id, Id, Id)>,
    ) -> Result<(MemoryReservation, usize), TransportError> {
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
        let memory = self.reserve(bytes)?;
        Ok((memory, bytes))
    }
    fn enqueue_packet(&mut self, packet: p::Packet) -> Result<(), TransportError> {
        let bytes = packet.encoded_len();
        let memory = self.reserve(bytes)?;
        self.push(packet, memory, bytes)
    }
    fn push(
        &mut self,
        packet: p::Packet,
        memory: MemoryReservation,
        bytes: usize,
    ) -> Result<(), TransportError> {
        if packet.encoded_len() != bytes {
            return Err(TransportError::Protocol);
        }
        let encoded = packet.encode_to_vec().into();
        self.entries.push_back(PendingReply {
            encoded,
            submitted: false,
            _memory: memory,
        });
        self.bytes += bytes;
        Ok(())
    }
    async fn flush<S: AsyncRead + AsyncWrite + Unpin>(
        &mut self,
        socket: &mut WebSocketStream<S>,
    ) -> Result<(), TransportError> {
        while let Some(reply) = self.entries.front_mut() {
            if !reply.submitted {
                socket
                    .feed(WsMessage::Binary(reply.encoded.clone()))
                    .await
                    .map_err(|_| TransportError::WebSocket)?;
                // feed completes synchronously with start_send. There is no
                // cancellation boundary between sink admission and this flag.
                reply.submitted = true;
            }
            socket
                .flush()
                .await
                .map_err(|_| TransportError::WebSocket)?;
            let reply = self.entries.pop_front().expect("flushed queue front");
            self.bytes -= reply.encoded.len();
        }
        Ok(())
    }
}
impl NetworkClient {
    pub async fn connect(
        address: SocketAddr,
        server_name: String,
        pinned_certificate: Vec<u8>,
        credential: &Credential,
        hello: ClientHello,
    ) -> Result<Self, TransportError> {
        if hello.resume.is_some() {
            return Err(TransportError::Protocol);
        }
        Self::connect_retained(
            address,
            server_name,
            pinned_certificate,
            credential,
            hello,
            None,
            false,
        )
        .await
    }
    async fn connect_retained(
        address: SocketAddr,
        server_name: String,
        pinned_certificate: Vec<u8>,
        credential: &Credential,
        mut hello: ClientHello,
        retained: Option<&mut RetainedClientView>,
        fresh: bool,
    ) -> Result<Self, TransportError> {
        let deadline = Duration::from_secs(5);
        // Only the authenticated explicit busy code is retryable. Generic
        // credential/pin/I/O failures leave the retained owner intact.
        let (socket, server, limits, context) = timeout(Duration::from_secs(2), async {
            loop {
                match Self::handshake(
                    address,
                    server_name.clone(),
                    pinned_certificate.clone(),
                    credential,
                    &mut hello,
                    fresh,
                )
                .await
                {
                    Err(TransportError::ProducerBusy) if retained.is_some() => {
                        tokio::time::sleep(Duration::from_millis(10)).await
                    }
                    result => break result,
                }
            }
        })
        .await
        .map_err(|_| TransportError::Deadline)??;
        if let Some(old) = retained.as_deref() {
            if (old.context == context
                && (old.limits != limits || old.profiles != server.selected_profiles))
                || (fresh && old.context == context)
            {
                return Err(TransportError::Protocol);
            }
        }
        let config = ReceiverConfig {
            single_glyph_outlines: server
                .selected_profiles
                .iter()
                .any(|f| f == "glyph.outline-single.v1"),
            ..Default::default()
        };
        let (receiver, pending) = if let Some(old) = retained {
            if old.context == context
                && old.limits == limits
                && old.profiles == server.selected_profiles
            {
                let receiver = old.receiver.as_ref().ok_or(TransportError::Receiver)?;
                let pending = PendingReplies::new(receiver.memory_budget(), &limits)?;
                old.receiver
                    .as_mut()
                    .unwrap()
                    .reconnect(context)
                    .map_err(|_| TransportError::Receiver)?;
                (old.receiver.take().unwrap(), pending)
            } else {
                let receiver = ViewReceiver::new(context, limits.clone(), config)
                    .map_err(|_| TransportError::Receiver)?;
                let pending = PendingReplies::new(receiver.memory_budget(), &limits)?;
                // Both allocations succeeded and a different scope is confirmed
                // before the former reducer/retired identities are disposed.
                old.receiver.take();
                (receiver, pending)
            }
        } else {
            let receiver = ViewReceiver::new(context, limits.clone(), config)
                .map_err(|_| TransportError::Receiver)?;
            let pending = PendingReplies::new(receiver.memory_budget(), &limits)?;
            (receiver, pending)
        };
        Ok(Self {
            socket,
            client_instance: hello.client_instance_id,
            session_id: context.scope.session_id,
            epoch: context.scope.epoch,
            view_id: context.view_id,
            limits,
            profiles: server.selected_profiles,
            context,
            receiver,
            needed: None,
            needed_requested: 0,
            announce_evictions: false,
            deadline,
            pending,
            awaiting_resume: hello.resume.as_ref().map(|r| r.acknowledged_revision),
            resume_applied: None,
            resume_needed: None,
        })
    }
    async fn handshake(
        address: SocketAddr,
        server_name: String,
        pinned_certificate: Vec<u8>,
        credential: &Credential,
        hello: &mut ClientHello,
        fresh_view: bool,
    ) -> Result<
        (
            ClientSocket,
            p::ServerHello,
            ReceiverLimits,
            ResourceContext,
        ),
        TransportError,
    > {
        let deadline = Duration::from_secs(5);
        let mut ceiling = hello.receiver_limits.envelope_bytes.min(1024 * 1024);
        if ceiling < websocket::MIN_MESSAGE {
            return Err(TransportError::Limit);
        }
        let mut connected = None;
        for _ in 0..2 {
            let stream = timeout(
                deadline,
                tls::connect_pinned(address, server_name.clone(), pinned_certificate.clone()),
            )
            .await
            .map_err(|_| TransportError::Deadline)??;
            let (socket, selected) = timeout(
                deadline,
                websocket::client_upgrade(stream, address, &server_name, ceiling),
            )
            .await
            .map_err(|_| TransportError::Deadline)??;
            if selected == ceiling {
                connected = Some(socket);
                break;
            }
            drop(socket);
            ceiling = selected;
        }
        let mut socket = connected.ok_or(TransportError::Protocol)?;
        hello.receiver_limits.envelope_bytes = ceiling;
        let mut wire = p::ClientHello {
            credential: credential.expose_for_tls_authentication().to_vec(),
            client_instance_id: hello.client_instance_id.to_vec(),
            supported_versions: hello.supported_versions.iter().copied().collect(),
            supported_profiles: hello.supported_profiles.iter().cloned().collect(),
            required_profiles: hello.required_profiles.iter().cloned().collect(),
            receiver_limits: Some(codec::wire_limits(&hello.receiver_limits)),
            fresh_view,
            resume: hello.resume.as_ref().map(|r| p::ResumeRequest {
                epoch: r.epoch.to_vec(),
                view_id: r.view_id.to_vec(),
                acknowledged_revision: r.acknowledged_revision,
            }),
        };
        websocket::send(
            &mut socket,
            codec::wrap(p::Kind::ClientHello, &wire, None),
            ceiling,
            deadline,
        )
        .await?;
        wire.credential.zeroize();
        let raw = timeout(deadline, websocket::receive(&mut socket))
            .await
            .map_err(|_| TransportError::Deadline)??;
        let initial: p::Packet = codec::decode(&raw, ceiling)?;
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
        let packet = codec::packet(&raw, ceiling, Some(binding))?;
        if packet.kind != p::Kind::ServerHello as i32 {
            return Err(TransportError::Protocol);
        }
        let server: p::ServerHello = codec::decode(&packet.payload, ceiling)?;
        let limits = codec::limits(server.negotiated_limits.ok_or(TransportError::Protocol)?)?;
        if limits.envelope_bytes != ceiling
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
        let context = ResourceContext {
            scope: ResourceScope {
                session_id: binding.0,
                epoch: binding.1,
                principal_id: codec::id(&server.principal_id)?,
            },
            view_id: binding.2,
        };
        Ok((socket, server, limits, context))
    }
    pub fn into_retained(self) -> RetainedClientView {
        let Self {
            socket,
            client_instance,
            context,
            limits,
            profiles,
            mut receiver,
            pending,
            needed,
            ..
        } = self;
        receiver.disconnect();
        // Retain the actual live closure; obsolete cache entries belong to the
        // former connection's eviction metadata and may be discarded locally.
        while receiver
            .evict_one_unpinned_resource()
            .ok()
            .flatten()
            .is_some()
        {}
        drop(needed);
        drop(pending);
        drop(socket);
        RetainedClientView {
            context,
            client_instance,
            limits,
            profiles,
            receiver: Some(receiver),
        }
    }
    fn binding(&self) -> Option<(Id, Id, Id)> {
        Some((self.session_id, self.epoch, self.view_id))
    }
    pub fn receiver(&self) -> &ViewReceiver {
        &self.receiver
    }
    fn queue<T: Message>(&mut self, kind: p::Kind, payload: &T) -> Result<(), TransportError> {
        self.pending.enqueue(kind, payload, self.binding())
    }
    /// Replies and their accounting are retained across cancellation. Sink
    /// admission is tracked separately from flushing, so resumption cannot
    /// silently omit a reply or enqueue its frame twice.
    pub async fn flush_pending(&mut self) -> Result<(), TransportError> {
        timeout(self.deadline, self.pending.flush(&mut self.socket))
            .await
            .map_err(|_| TransportError::Deadline)?
    }
    pub fn pending_outbound_packets(&self) -> usize {
        self.pending.entries.len()
    }
    /// Low-level binary binding used by adapters and conformance tests. The host
    /// validates direction, kind, binding and authority for every packet.
    pub async fn send_packet(&mut self, packet: p::Packet) -> Result<(), TransportError> {
        self.pending.enqueue_packet(packet)?;
        self.flush_pending().await
    }
    pub async fn send<T: Message>(
        &mut self,
        kind: p::Kind,
        payload: &T,
    ) -> Result<(), TransportError> {
        self.queue(kind, payload)?;
        self.flush_pending().await
    }
    pub async fn open_view(&mut self) -> Result<(), TransportError> {
        self.send(
            p::Kind::OpenView,
            &p::OpenView {
                selection: "fixture".into(),
                mirror_canvas_policy: if self.profiles.iter().any(|p| p == "semantics.only.v1") {
                    "semantic-only"
                } else {
                    "host-canvas"
                }
                .into(),
            },
        )
        .await
    }
    pub async fn acquire_control(&mut self, request: Id) -> Result<(), TransportError> {
        self.send(
            p::Kind::AcquireControl,
            &p::AcquireControl {
                request_id: request.to_vec(),
            },
        )
        .await
    }
    pub async fn submit(&mut self, exact_command_bytes: &[u8]) -> Result<(), TransportError> {
        self.submit_family(CommandFamily::Semantic, exact_command_bytes)
            .await
    }
    /// Send exact command bytes in the common producer sequence space. The host
    /// binds the family to its decoder and performs all authority checks.
    pub async fn submit_family(
        &mut self,
        family: CommandFamily,
        exact_command_bytes: &[u8],
    ) -> Result<(), TransportError> {
        if exact_command_bytes.len() > self.limits.envelope_bytes.saturating_sub(128) {
            return Err(TransportError::Limit);
        }
        self.send_packet(codec::wrap_bytes(
            match family {
                CommandFamily::Semantic => p::Kind::SemanticCommand,
                CommandFamily::RawInput => p::Kind::RawInputBatch,
                CommandFamily::TextComposition => p::Kind::TextComposition,
                CommandFamily::Resize => p::Kind::ResizeRequest,
            },
            exact_command_bytes.to_vec(),
            self.binding(),
        ))
        .await
    }
    /// Trusted renderer completion callback; this is never called automatically
    /// merely because application state was received.
    pub async fn presented(&mut self, revision: u64) -> Result<(), TransportError> {
        if self.profiles.iter().any(|p| p == "semantics.only.v1") {
            return Err(TransportError::Authority);
        }
        self.receiver
            .mark_presented(revision)
            .map_err(|_| TransportError::Receiver)?;
        self.send(p::Kind::PresentedAck, &p::RevisionAck { revision })
            .await
    }
    /// Semantic-only consumption is explicit and never calls mark_presented.
    pub async fn semantic_ready(&mut self, revision: u64) -> Result<(), TransportError> {
        if !self.profiles.iter().any(|p| p == "semantics.only.v1")
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
        self.send(p::Kind::SemanticReady, &p::RevisionAck { revision })
            .await
    }
    pub async fn close(mut self) -> Result<(), TransportError> {
        self.send(
            p::Kind::CloseView,
            &p::Error {
                code: String::new(),
            },
        )
        .await
    }
    fn outcome(&mut self, outcome: ReceiveOutcome) -> Result<ClientEvent, TransportError> {
        match outcome {
            ReceiveOutcome::Applied { revision } | ReceiveOutcome::Duplicate { revision } => {
                self.needed = None;
                self.needed_requested = 0;
                self.announce_evictions = false;
                self.queue(p::Kind::AppliedAck, &p::RevisionAck { revision })?;
                Ok(ClientEvent::Applied { revision })
            }
            ReceiveOutcome::NeedResources {
                revision, missing, ..
            } => {
                self.needed = Some(missing);
                self.needed_requested = 0;
                self.queue_resource_request()?;
                Ok(ClientEvent::ResourcesNeeded { revision })
            }
        }
    }
    fn queue_resource_request(&mut self) -> Result<(), TransportError> {
        let Some(needed) = &self.needed else {
            return Ok(());
        };
        if self.needed_requested >= needed.len() {
            return Ok(());
        }
        let maximum = self.limits.envelope_bytes.saturating_sub(256) / 40;
        if maximum == 0 || needed.len() > 512 {
            return Err(TransportError::Limit);
        }
        let start = self.needed_requested;
        let count = (needed.len() - start).min(maximum);
        let binding = self.binding();
        if self.announce_evictions {
            self.pending
                .enqueue_with(p::Kind::ResourceEvicted, count * 34, binding, || {
                    p::ResourceIds {
                        digests: needed
                            .iter()
                            .skip(start)
                            .take(count)
                            .map(|d| d.to_vec())
                            .collect(),
                    }
                })?;
        }
        self.pending
            .enqueue_with(p::Kind::NeedResources, count * 34, binding, || {
                p::ResourceIds {
                    digests: needed
                        .iter()
                        .skip(start)
                        .take(count)
                        .map(|d| d.to_vec())
                        .collect(),
                }
            })?;
        self.needed_requested += count;
        Ok(())
    }
    /// Ordinary cache pressure releases one retired, unpinned entry. Current
    /// graph and candidate dependencies cannot be evicted by this operation.
    /// Cancellation retains the queued notification for the next flush.
    pub async fn evict_unpinned_resource(&mut self) -> Result<Option<[u8; 32]>, TransportError> {
        let binding = self.binding();
        let (memory, bytes) =
            self.pending
                .reserve_message(p::Kind::ResourceEvicted, 34, binding)?;
        let Some(evicted) = self
            .receiver
            .evict_one_unpinned_resource()
            .map_err(|_| TransportError::Receiver)?
        else {
            return Ok(None);
        };
        let digest = evicted.digest;
        self.pending.push(
            codec::wrap(
                p::Kind::ResourceEvicted,
                &p::ResourceIds {
                    digests: vec![digest.to_vec()],
                },
                binding,
            ),
            memory,
            bytes,
        )?;
        self.flush_pending().await?;
        Ok(Some(digest))
    }
    /// Trusted local cache-loss signal. It pauses presentation immediately;
    /// resource recovery is requested in bounded pages and APPLIED is re-sent
    /// only after the live closure is verified. A pending candidate must first
    /// abort/reconnect rather than mixing its receiving transfer with cache loss.
    pub async fn resource_cache_lost(&mut self) -> Result<(), TransportError> {
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
            self.queue(p::Kind::AppliedAck, &p::RevisionAck { revision })?;
        } else {
            self.needed = Some(missing);
            self.needed_requested = 0;
            self.announce_evictions = true;
            self.queue_resource_request()?;
        }
        self.flush_pending().await
    }
    /// Cancellation-safe for receiver state and observable events: every received
    /// packet is handled synchronously through its event return. Required replies
    /// are queued before returning; the next receive/send flushes them reliably.
    pub async fn receive(&mut self) -> Result<ClientEvent, TransportError> {
        loop {
            if let Some(revision) = self.resume_needed.take() {
                return Ok(ClientEvent::ResourcesNeeded { revision });
            }
            if let Some(revision) = self.resume_applied.take() {
                return self.outcome(ReceiveOutcome::Applied { revision });
            }
            self.queue_resource_request()?;
            self.flush_pending().await?;
            let raw = timeout(self.deadline, websocket::receive(&mut self.socket))
                .await
                .map_err(|_| TransportError::Deadline)??;
            let packet = codec::packet(&raw, self.limits.envelope_bytes, self.binding())?;
            let kind = p::Kind::try_from(packet.kind).map_err(|_| TransportError::Protocol)?;
            let ceiling = self.limits.envelope_bytes;
            let context = self.context;
            match kind {
                p::Kind::ViewOpened => {
                    return Ok(ClientEvent::ViewOpened(codec::decode(
                        &packet.payload,
                        ceiling,
                    )?))
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
                    let outcome = self
                        .receiver
                        .commit_authenticated_publication(
                            context,
                            id,
                            codec::publication_commit(commit)?,
                        )
                        .map_err(|_| TransportError::Receiver)?;
                    return self.outcome(outcome);
                }
                p::Kind::AbortPublication => {
                    let abort: p::AbortPublication = codec::decode(&packet.payload, ceiling)?;
                    self.receiver
                        .abort_publication(context, codec::id(&abort.publication_id)?)
                        .map_err(|_| TransportError::Receiver)?;
                }
                p::Kind::ResourceOffer => {
                    let offer = codec::resource_offer(codec::decode(&packet.payload, ceiling)?)?;
                    if !self
                        .needed
                        .as_ref()
                        .is_some_and(|needed| needed.contains(&offer.digest))
                    {
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
                    self.pending.enqueue_with(
                        p::Kind::ResourceAvailable,
                        34,
                        self.binding(),
                        || p::ResourceIds {
                            digests: vec![digest.to_vec()],
                        },
                    )?;
                    if let Some(outcome) = outcome {
                        // The original, charged request set/cursor already
                        // covers the whole candidate closure. A partial retry
                        // reports a smaller missing set, not a new publication
                        // or a reason to request completed resources again.
                        if matches!(outcome, ReceiveOutcome::NeedResources { .. }) {
                            return Ok(ClientEvent::ResourceAvailable);
                        }
                        return self.outcome(outcome);
                    }
                    if self.needed.is_some()
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
                        return self.outcome(ReceiveOutcome::Applied { revision });
                    }
                    return Ok(ClientEvent::ResourceAvailable);
                }
                p::Kind::ControlGranted => {
                    return Ok(ClientEvent::ControlGranted(codec::decode(
                        &packet.payload,
                        ceiling,
                    )?))
                }
                p::Kind::ControlRevoked => {
                    return Ok(ClientEvent::ControlRevoked(codec::decode(
                        &packet.payload,
                        ceiling,
                    )?))
                }
                p::Kind::CommandResult => {
                    return Ok(ClientEvent::CommandResult(codec::decode(
                        &packet.payload,
                        ceiling,
                    )?))
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
                            self.needed = Some(missing);
                            self.needed_requested = 0;
                            self.announce_evictions = true;
                            self.resume_needed = Some(revision);
                            self.queue_resource_request()?;
                        }
                    }
                    return Ok(ClientEvent::ResumeAccepted(resumed));
                }
                p::Kind::ResetRequired => {
                    if self.awaiting_resume.take().is_none() {
                        return Err(TransportError::Protocol);
                    }
                    // A higher checkpoint preserves this retained reducer's
                    // retirement and monotonic clocks under the same view.
                    return Ok(ClientEvent::ResetRequired(codec::decode(
                        &packet.payload,
                        ceiling,
                    )?));
                }
                p::Kind::Error => {
                    let error: p::Error = codec::decode(&packet.payload, ceiling)?;
                    return Ok(ClientEvent::Error(error.code));
                }
                p::Kind::Pong => {
                    let pong: p::Ping = codec::decode(&packet.payload, ceiling)?;
                    return Ok(ClientEvent::Pong(pong.nonce));
                }
                _ => return Err(TransportError::Protocol),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};

    fn limits() -> ReceiverLimits {
        ReceiverLimits {
            envelope_bytes: 4096,
            outbound_queue_bytes: 4096,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn cancelled_polled_flush_retains_charge_and_resumes_exactly_one_frame() {
        let budget = MemoryBudget::new(1024 * 1024);
        let mut pending = PendingReplies::new(budget.clone(), &limits()).unwrap();
        let writer_charge = budget.used();
        pending
            .enqueue_with(
                p::Kind::AppliedAck,
                2,
                Some(([1; 16], [2; 16], [3; 16])),
                || p::RevisionAck { revision: 1 },
            )
            .unwrap();
        let queued_charge = budget.used();
        assert!(queued_charge > writer_charge && queued_charge <= budget.limit());
        let (writer, reader) = tokio::io::duplex(16);
        let config = WebSocketConfig::default()
            .write_buffer_size(0)
            .max_write_buffer_size(8320)
            .max_message_size(Some(4096))
            .max_frame_size(Some(4096));
        let mut socket = WebSocketStream::from_raw_socket(writer, Role::Client, Some(config)).await;
        let mut peer = WebSocketStream::from_raw_socket(reader, Role::Server, Some(config)).await;

        // This polls feed/start_send and then blocks on the 16-byte duplex pipe.
        assert!(
            timeout(Duration::from_millis(10), pending.flush(&mut socket))
                .await
                .is_err()
        );
        assert!(pending.entries.front().unwrap().submitted);
        assert_eq!(pending.entries.len(), 1);
        assert_eq!(budget.used(), queued_charge);

        let (flushed, frame) = timeout(Duration::from_secs(1), async {
            tokio::join!(pending.flush(&mut socket), peer.next())
        })
        .await
        .expect("resume and peer read must finish");
        flushed.unwrap();
        let WsMessage::Binary(bytes) = frame.unwrap().unwrap() else {
            panic!("binary ACK required")
        };
        let packet: p::Packet = codec::decode(&bytes, 4096).unwrap();
        assert_eq!(packet.kind, p::Kind::AppliedAck as i32);
        let ack: p::RevisionAck = codec::decode(&packet.payload, 4096).unwrap();
        assert_eq!(ack.revision, 1);
        assert!(pending.entries.is_empty());
        assert_eq!(budget.used(), writer_charge);
        assert!(timeout(Duration::from_millis(10), peer.next())
            .await
            .is_err());
        drop(socket);
        drop(pending);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn shared_budget_exhaustion_rejects_before_payload_allocation() {
        let negotiated = limits();
        let writer_charge = negotiated.envelope_bytes * 4 + 64 * 1024;
        let budget = MemoryBudget::new(writer_charge + 1);
        let mut pending = PendingReplies::new(budget.clone(), &negotiated).unwrap();
        let called = std::cell::Cell::new(false);
        assert_eq!(
            pending.enqueue_with(
                p::Kind::AppliedAck,
                2,
                Some(([1; 16], [2; 16], [3; 16])),
                || {
                    called.set(true);
                    p::RevisionAck { revision: 1 }
                }
            ),
            Err(TransportError::Limit)
        );
        assert!(!called.get());
        assert!(pending.entries.is_empty());
        assert_eq!(budget.used(), writer_charge);
        drop(pending);
        assert_eq!(budget.used(), 0);
        assert!(matches!(
            PendingReplies::new(MemoryBudget::new(writer_charge - 1), &negotiated),
            Err(TransportError::Limit)
        ));
    }
}
