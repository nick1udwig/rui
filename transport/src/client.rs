use crate::{codec, pairing::Credential, proto as p, tls, websocket, TransportError};
use futures_util::SinkExt;
use prost::Message;
use rui_protocol_core::commands::{CommandFamily, Id};
use rui_protocol_core::receiver::{ReceiverConfig, ViewReceiver};
use rui_protocol_core::resources::{MemoryBudget, MemoryReservation, ResourceContext};
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

pub use rui_client_core::replica::{ClientEvent, OpenedView};
use rui_client_core::replica::{Replica, Replies};

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
    commands: Option<rui_client_core::command_queue::CommandQueue>,
}
impl RetainedClientView {
    pub fn receiver(&self) -> Option<&ViewReceiver> {
        self.receiver.as_ref()
    }
    pub fn commands(&self) -> Option<&rui_client_core::command_queue::CommandQueue> {
        self.commands.as_ref()
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
    replica: Replica,
    deadline: Duration,
    pending: PendingReplies,
}

struct PendingReply {
    timing: Option<rui_protocol_core::timing::Span>,
    encoded: Bytes,
    submitted: bool,
    _memory: MemoryReservation,
}
impl Drop for PendingReply {
    fn drop(&mut self) {
        if let Some(timing) = self.timing.as_mut() {
            timing.discard();
        }
    }
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
    #[cfg(test)]
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
        let bytes = rui_client_core::replica::message_size(kind, payload_len, binding)?;
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
            timing: Some(rui_protocol_core::timing::span(
                rui_protocol_core::timing::Stage::OutboundQueue,
                0,
                bytes,
            )),
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
                reply.timing.take();
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
        let maximum = limits.envelope_bytes.saturating_sub(128);
        let (receiver, pending, commands, capabilities) = if let Some(old) = retained {
            if old.context == context
                && old.limits == limits
                && old.profiles == server.selected_profiles
            {
                let receiver = old.receiver.as_ref().ok_or(TransportError::Receiver)?;
                let pending = PendingReplies::new(receiver.memory_budget(), &limits)?;
                let capabilities = rui_client_core::replica::reserve_capabilities(
                    &server.selected_profiles,
                    server
                        .permissions
                        .as_ref()
                        .ok_or(TransportError::Protocol)?,
                    receiver.memory_budget(),
                )?;
                old.receiver
                    .as_mut()
                    .unwrap()
                    .reconnect(context)
                    .map_err(|_| TransportError::Receiver)?;
                (
                    old.receiver.take().unwrap(),
                    pending,
                    old.commands.take().unwrap(),
                    capabilities,
                )
            } else {
                let receiver = ViewReceiver::new(context, limits.clone(), config)
                    .map_err(|_| TransportError::Receiver)?;
                let pending = PendingReplies::new(receiver.memory_budget(), &limits)?;
                let capabilities = rui_client_core::replica::reserve_capabilities(
                    &server.selected_profiles,
                    server
                        .permissions
                        .as_ref()
                        .ok_or(TransportError::Protocol)?,
                    receiver.memory_budget(),
                )?;
                let commands = old.commands.as_mut().ok_or(TransportError::Receiver)?;
                commands
                    .rebind(context.scope.epoch, receiver.memory_budget(), maximum)
                    .map_err(|_| TransportError::Limit)?;
                // All allocations succeed before the former owner is consumed.
                old.receiver.take();
                (
                    receiver,
                    pending,
                    old.commands.take().unwrap(),
                    capabilities,
                )
            }
        } else {
            let receiver = ViewReceiver::new(context, limits.clone(), config)
                .map_err(|_| TransportError::Receiver)?;
            let pending = PendingReplies::new(receiver.memory_budget(), &limits)?;
            let capabilities = rui_client_core::replica::reserve_capabilities(
                &server.selected_profiles,
                server
                    .permissions
                    .as_ref()
                    .ok_or(TransportError::Protocol)?,
                receiver.memory_budget(),
            )?;
            let commands = rui_client_core::command_queue::CommandQueue::new(
                context.scope.epoch,
                receiver.memory_budget(),
                maximum,
            )
            .map_err(|_| TransportError::Limit)?;
            (receiver, pending, commands, capabilities)
        };
        let mut replica = Replica::with_commands(
            context,
            limits.clone(),
            server
                .selected_profiles
                .iter()
                .any(|p| p == "semantics.only.v1"),
            receiver,
            hello.resume.as_ref().map(|r| r.acknowledged_revision),
            commands,
        );
        replica.install_capabilities(capabilities);
        Ok(Self {
            socket,
            client_instance: hello.client_instance_id,
            session_id: context.scope.session_id,
            epoch: context.scope.epoch,
            view_id: context.view_id,
            limits: limits.clone(),
            replica,
            profiles: server.selected_profiles,
            context,
            deadline,
            pending,
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
        let rui_client_core::replica::Negotiated {
            server,
            limits,
            context,
        } = rui_client_core::replica::negotiate_server(&raw, hello, true)?;
        Ok((socket, server, limits, context))
    }
    pub fn into_retained(self) -> RetainedClientView {
        let Self {
            socket,
            client_instance,
            context,
            limits,
            profiles,
            replica,
            pending,
            ..
        } = self;
        let (mut receiver, commands) = replica.into_parts();
        receiver.disconnect();
        // Retain the actual live closure; obsolete cache entries belong to the
        // former connection's eviction metadata and may be discarded locally.
        while receiver
            .evict_one_unpinned_resource()
            .ok()
            .flatten()
            .is_some()
        {}
        drop(pending);
        drop(socket);
        RetainedClientView {
            context,
            client_instance,
            limits,
            profiles,
            receiver: Some(receiver),
            commands: Some(commands),
        }
    }
    fn binding(&self) -> Option<(Id, Id, Id)> {
        Some((self.session_id, self.epoch, self.view_id))
    }
    pub fn receiver(&self) -> &ViewReceiver {
        self.replica.receiver()
    }
    fn queue<T: Message>(&mut self, kind: p::Kind, payload: &T) -> Result<(), TransportError> {
        if !self.replica.is_connected() {
            return Err(TransportError::Closed);
        }
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
        if !self.replica.is_connected() {
            return Err(TransportError::Closed);
        }
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
        let policy = if self.profiles.iter().any(|p| p == "semantics.only.v1") {
            "semantic-only"
        } else {
            "host-canvas"
        };
        self.open_view_with("fixture", policy).await
    }
    /// Request an explicit producer selection. The authenticated producer must
    /// approve this scope; the transport never aliases it to a wider view.
    pub async fn open_view_with(
        &mut self,
        selection: &str,
        canvas_policy: &str,
    ) -> Result<(), TransportError> {
        if selection.is_empty()
            || selection.len() > 256
            || canvas_policy.is_empty()
            || canvas_policy.len() > 256
        {
            return Err(TransportError::Limit);
        }
        self.send(
            p::Kind::OpenView,
            &p::OpenView {
                selection: selection.into(),
                mirror_canvas_policy: canvas_policy.into(),
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
    pub fn commands(&self) -> &rui_client_core::command_queue::CommandQueue {
        self.replica.commands()
    }
    /// Native builders encode directly into the actor-owned exact ledger.
    pub async fn submit_encoded<T: Message>(
        &mut self,
        family: CommandFamily,
        wire: &T,
    ) -> Result<(), TransportError> {
        if !self.replica.is_connected() {
            return Err(TransportError::Closed);
        }
        self.replica
            .commands_mut()
            .encode(family, wire.encoded_len(), |b| {
                wire.encode(b).map_err(|_| ())
            })
            .map_err(|_| TransportError::Limit)?;
        let command = self.replica.commands().latest().unwrap();
        let bytes = command.bytes().to_vec();
        self.send_exact(family, bytes).await
    }
    /// Retry one original family/sequence/authority envelope. This never rebuilds an edit.
    pub async fn retry_command(&mut self, sequence: u64) -> Result<(), TransportError> {
        let command = self
            .replica
            .commands()
            .commands()
            .find(|c| c.sequence() == sequence)
            .ok_or(TransportError::Protocol)?;
        let family = command.family();
        let bytes = command.bytes().to_vec();
        self.send_exact(family, bytes).await
    }
    async fn send_exact(
        &mut self,
        family: CommandFamily,
        bytes: Vec<u8>,
    ) -> Result<(), TransportError> {
        self.send_packet(codec::wrap_bytes(
            match family {
                CommandFamily::Semantic => p::Kind::SemanticCommand,
                CommandFamily::RawInput => p::Kind::RawInputBatch,
                CommandFamily::TextComposition => p::Kind::TextComposition,
                CommandFamily::Resize => p::Kind::ResizeRequest,
            },
            bytes,
            self.binding(),
        ))
        .await
    }
    pub async fn submit(&mut self, exact_command_bytes: &[u8]) -> Result<(), TransportError> {
        self.submit_family(CommandFamily::Semantic, exact_command_bytes)
            .await
    }
    /// Query this authenticated producer's retained common-ledger outcome.
    /// This neither redispatches the command nor restores its former control.
    pub async fn get_command_result(&mut self, sequence: u64) -> Result<(), TransportError> {
        if sequence == 0 {
            return Err(TransportError::Protocol);
        }
        self.send(
            p::Kind::GetCommandResult,
            &p::GetCommandResult {
                command_seq: sequence,
            },
        )
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
        self.replica.retain_command(family, exact_command_bytes)?;
        self.send_exact(family, exact_command_bytes.to_vec()).await
    }
    /// Trusted renderer completion callback; this is never called automatically
    /// merely because application state was received.
    pub async fn presented(&mut self, revision: u64) -> Result<(), TransportError> {
        self.replica.presented(revision, &mut self.pending)?;
        self.flush_pending().await
    }
    pub async fn semantic_ready(&mut self, revision: u64) -> Result<(), TransportError> {
        self.replica.semantic_ready(revision, &mut self.pending)?;
        self.flush_pending().await
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
    pub async fn evict_unpinned_resource(&mut self) -> Result<Option<[u8; 32]>, TransportError> {
        let digest = self.replica.evict_unpinned_resource(&mut self.pending)?;
        self.flush_pending().await?;
        Ok(digest)
    }
    pub async fn resource_cache_lost(&mut self) -> Result<(), TransportError> {
        self.replica.resource_cache_lost(&mut self.pending)?;
        self.flush_pending().await
    }
    /// Decoding and event/reply admission are synchronous once an awaited packet
    /// arrives. The sink's submitted marker survives cancellation of flushing.
    pub async fn receive(&mut self) -> Result<ClientEvent, TransportError> {
        if !self.replica.is_connected() {
            return Err(TransportError::Closed);
        }
        loop {
            if let Some(event) = self.replica.poll(&mut self.pending)? {
                return Ok(event);
            }
            self.replica.queue_resource_request(&mut self.pending)?;
            self.flush_pending().await?;
            let raw = timeout(self.deadline, websocket::receive(&mut self.socket))
                .await
                .map_err(|_| TransportError::Deadline)??;
            if let Some(event) = self.replica.receive(&raw, &mut self.pending)? {
                return Ok(event);
            }
        }
    }
}
impl Replies for PendingReplies {
    type Reservation = (MemoryReservation, usize);
    fn reserve(
        &self,
        kind: p::Kind,
        length: usize,
        binding: Option<(Id, Id, Id)>,
    ) -> Result<Self::Reservation, TransportError> {
        self.reserve_message(kind, length, binding)
    }
    fn enqueue_reserved<T: Message>(
        &mut self,
        kind: p::Kind,
        binding: Option<(Id, Id, Id)>,
        (memory, bytes): Self::Reservation,
        make: impl FnOnce() -> T,
    ) -> Result<(), TransportError> {
        self.push(codec::wrap(kind, &make(), binding), memory, bytes)
    }
    fn enqueue<T: Message>(
        &mut self,
        kind: p::Kind,
        payload: &T,
        binding: Option<(Id, Id, Id)>,
    ) -> Result<(), TransportError> {
        PendingReplies::enqueue(self, kind, payload, binding)
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
