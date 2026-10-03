use crate::diagnostics::{Diagnostics, RecordingSession};
use crate::producer::{
    self, Acknowledgement, Connection, Control, Operation, ProducerPort, Reply, Request,
};
use crate::source_delivery::Delivery;
use crate::{
    codec, pairing::PairingAuthority, proto as p, websocket, HostIdentity, TransportError,
};
use futures_util::{Sink, Stream, StreamExt};
use prost::Message;
use rui_host_broker::ControlledHost;
use rui_protocol_core::commands::Id;
use rui_protocol_core::publication::publication_digest;
use rui_protocol_core::resources::{MemoryBudget, MemoryReservation};
use rui_protocol_core::session::{
    InputCleanup, ResumeDisposition, SharedPayload, ViewMemoryReservation,
};
use rui_protocol_core::transcript::Direction;
use std::{
    collections::{BTreeSet, VecDeque},
    net::SocketAddr,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, Mutex, Semaphore},
    task::{JoinHandle, JoinSet},
};
use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message as WebSocketMessage};
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone)]
pub struct ServerConfig {
    pub diagnostics: Option<Arc<Diagnostics>>,
    pub max_connections: usize,
    pub max_message_bytes: usize,
    pub handshake_timeout: Duration,
    pub send_timeout: Duration,
    pub publication_timeout: Duration,
    /// Legacy ControlledHost cleanup adapter. Native producer ports own the
    /// corresponding complete cleanup transaction themselves.
    pub cleanup: Arc<dyn Fn(InputCleanup) + Send + Sync>,
}
impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            diagnostics: None,
            max_connections: 16,
            max_message_bytes: 1024 * 1024,
            handshake_timeout: Duration::from_secs(5),
            send_timeout: Duration::from_secs(2),
            publication_timeout: Duration::from_secs(30),
            cleanup: Arc::new(|cleanup| {
                assert!(
                    cleanup.keys.is_empty()
                        && cleanup.buttons.is_empty()
                        && cleanup.touches.is_empty()
                        && cleanup.composition.is_none(),
                    "input cleanup adapter required"
                );
            }),
        }
    }
}
struct Shared {
    port: Arc<dyn ProducerPort>,
    pairing: Arc<StdMutex<PairingAuthority>>,
    active: StdMutex<BTreeSet<Id>>,
    start: Instant,
    config: ServerConfig,
}
impl Shared {
    fn now(&self) -> u64 {
        self.start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
    }
}
struct Runtime {
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}
impl Runtime {
    async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}
/// Backwards-compatible fixture host handle for trusted source tests.
pub struct ServerHandle {
    pub address: SocketAddr,
    pub host: Arc<Mutex<ControlledHost>>,
    runtime: Runtime,
}
impl ServerHandle {
    pub async fn bind(
        address: SocketAddr,
        identity: HostIdentity,
        host: ControlledHost,
        pairing: Arc<StdMutex<PairingAuthority>>,
        config: ServerConfig,
    ) -> Result<Self, TransportError> {
        let host = Arc::new(Mutex::new(host));
        let port = producer::ControlledPort::new(
            host.clone(),
            config.cleanup.clone(),
            config.max_connections,
        )?;
        let server =
            ProducerServerHandle::bind(address, identity, Arc::new(port), pairing, config).await?;
        Ok(Self {
            address: server.address,
            host,
            runtime: server.runtime,
        })
    }
    pub async fn shutdown(self) {
        self.runtime.shutdown().await;
    }
}
/// The same pinned TLS/framing/recording path with an independently owned source.
pub struct ProducerServerHandle {
    pub address: SocketAddr,
    runtime: Runtime,
}
impl ProducerServerHandle {
    pub async fn bind(
        address: SocketAddr,
        identity: HostIdentity,
        port: Arc<dyn ProducerPort>,
        pairing: Arc<StdMutex<PairingAuthority>>,
        config: ServerConfig,
    ) -> Result<Self, TransportError> {
        if config.max_connections == 0
            || config.max_connections > 128
            || config.max_message_bytes < websocket::MIN_MESSAGE
            || config.max_message_bytes > 1024 * 1024
            || config.handshake_timeout.is_zero()
            || config.send_timeout.is_zero()
            || config.publication_timeout.is_zero()
        {
            return Err(TransportError::Limit);
        }
        let listener = TcpListener::bind(address)
            .await
            .map_err(|_| TransportError::Io)?;
        let address = listener.local_addr().map_err(|_| TransportError::Io)?;
        let shared = Arc::new(Shared {
            port,
            pairing,
            active: StdMutex::new(BTreeSet::new()),
            start: Instant::now(),
            config: config.clone(),
        });
        let (stop, mut stopped) = oneshot::channel();
        let acceptor = identity.acceptor();
        let permits = Arc::new(Semaphore::new(config.max_connections));
        let task = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            loop {
                tokio::select! {
                    _=&mut stopped=>break,
                    Some(_)=tasks.join_next(),if !tasks.is_empty()=>{},
                    accepted=listener.accept()=>{let Ok((tcp,_))=accepted else{break;};let Ok(permit)=permits.clone().try_acquire_owned()else{drop(tcp);continue;};
                        let shared=shared.clone();let acceptor=acceptor.clone();tasks.spawn(async move{let _permit=permit;let _=connection(tcp,acceptor,shared).await;});}
                }
            }
            shared.port.shutdown();
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            let _ = tokio::time::timeout(
                Duration::from_secs(3),
                shared.port.flush_lifecycle(shared.now()),
            )
            .await;
        });
        Ok(Self {
            address,
            runtime: Runtime {
                stop: Some(stop),
                task: Some(task),
            },
        })
    }
    pub async fn shutdown(self) {
        self.runtime.shutdown().await;
    }
}
struct ActiveGuard {
    shared: Arc<Shared>,
    id: Id,
}
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = self.shared.active.lock() {
            active.remove(&self.id);
        }
    }
}
struct ConnectionGuard {
    port: Arc<dyn ProducerPort>,
    connection: Connection,
}
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.port.disconnect(&self.connection);
    }
}
async fn connection(
    tcp: TcpStream,
    acceptor: tokio_rustls::TlsAcceptor,
    shared: Arc<Shared>,
) -> Result<(), TransportError> {
    tcp.set_nodelay(true).map_err(|_| TransportError::Io)?;
    let stream = tokio::time::timeout(shared.config.handshake_timeout, acceptor.accept(tcp))
        .await
        .map_err(|_| TransportError::Deadline)?
        .map_err(|_| TransportError::Tls)?;
    let (mut socket, ceiling) = tokio::time::timeout(
        shared.config.handshake_timeout,
        websocket::server_upgrade(stream, shared.config.max_message_bytes),
    )
    .await
    .map_err(|_| TransportError::Deadline)??;
    let mut raw = zeroize::Zeroizing::new(
        tokio::time::timeout(
            shared.config.handshake_timeout,
            websocket::receive(&mut socket),
        )
        .await
        .map_err(|_| TransportError::Deadline)??,
    );
    if raw.len() > 64 * 1024 {
        return Err(TransportError::Limit);
    }
    let mut packet = codec::packet(&raw, ceiling, None)?;
    if packet.kind != p::Kind::ClientHello as i32 {
        return Err(TransportError::Authentication);
    }
    let mut wire: p::ClientHello = codec::decode(&packet.payload, 64 * 1024)?;
    let client_id = codec::id(&wire.client_instance_id)?;
    let (authorization, approval, lifetime) = {
        let pairing = shared
            .pairing
            .lock()
            .map_err(|_| TransportError::Authentication)?;
        (
            pairing.authenticated(&wire.credential, client_id)?,
            pairing.approval_id(&wire.credential, client_id)?,
            pairing.approval_lifetime(&wire.credential, client_id)?,
        )
    };
    wire.credential.zeroize();
    packet.payload.zeroize();
    raw.zeroize();
    let fresh_view = wire.fresh_view;
    let mut hello = codec::hello(wire)?;
    if hello.receiver_limits.envelope_bytes < ceiling {
        return Err(TransportError::Limit);
    }
    hello.receiver_limits.envelope_bytes = ceiling;
    let busy = {
        let mut active = shared
            .active
            .lock()
            .map_err(|_| TransportError::Authentication)?;
        !active.insert(client_id)
    };
    if busy {
        websocket::send(
            &mut socket,
            codec::wrap(
                p::Kind::Error,
                &p::Error {
                    code: "PRODUCER_CONNECTION_BUSY".into(),
                },
                None,
            ),
            ceiling,
            shared.config.send_timeout,
        )
        .await?;
        return Err(TransportError::ProducerBusy);
    }
    let _active = ActiveGuard {
        shared: shared.clone(),
        id: client_id,
    };
    if !shared
        .pairing
        .lock()
        .map_err(|_| TransportError::Authentication)?
        .remains_authorized(approval, client_id, &authorization)
    {
        return Err(TransportError::Authentication);
    }
    let received = shared
        .port
        .transact(Request::Connect {
            authorization: authorization.clone(),
            hello: Box::new(hello),
            lifetime: lifetime.clone(),
            fresh_view,
            now: shared.now(),
        })
        .await?;
    let Reply::Connected(connection) = received.value() else {
        return Err(TransportError::Protocol);
    };
    let connection = connection.clone();
    drop(received);
    let _connection = ConnectionGuard {
        port: shared.port.clone(),
        connection: connection.clone(),
    };
    let recording = if let Some(owner) = &shared.config.diagnostics {
        let memory = reserve(&shared, &connection, owner.reservation_bytes()).await?;
        Some(owner.open(connection.session(), lifetime, memory)?)
    } else {
        None
    };
    let result = run_authenticated(
        socket,
        &shared,
        &connection,
        ceiling,
        approval,
        &authorization,
        recording,
    )
    .await;
    shared.port.disconnect(&connection);
    let _ = shared.port.flush_lifecycle(shared.now()).await;
    result
}
async fn request(
    shared: &Shared,
    connection: &Connection,
    operation: Operation,
) -> Result<producer::Received, TransportError> {
    shared
        .port
        .transact(Request::Scoped {
            connection: connection.clone(),
            operation,
            now: shared.now(),
        })
        .await
}
async fn reserve(
    shared: &Shared,
    connection: &Connection,
    bytes: usize,
) -> Result<rui_protocol_core::session::ViewMemoryReservation, TransportError> {
    request(shared, connection, Operation::ReserveMemory { bytes })
        .await?
        .into_memory()
}
// Field order keeps every socket allocation covered until its own destruction.
// This owner is disposed before the caller awaits native lifecycle completion.
struct AuthenticatedSocket<S> {
    socket: tokio_tungstenite::WebSocketStream<S>,
    io_memory: Option<ViewMemoryReservation>,
    ingress_memory: Option<ViewMemoryReservation>,
}
#[allow(clippy::too_many_arguments)]
async fn run_authenticated<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    socket: tokio_tungstenite::WebSocketStream<S>,
    shared: &Shared,
    connection: &Connection,
    ceiling: usize,
    approval: [u8; 32],
    authorization: &rui_protocol_core::session::VerifiedAuthorization,
    recording: Option<RecordingSession>,
) -> Result<(), TransportError> {
    let mut socket = AuthenticatedSocket {
        socket,
        io_memory: None,
        ingress_memory: None,
    };
    let session = connection.session();
    if session.limits.envelope_bytes != ceiling {
        return Err(TransportError::Limit);
    }
    approved(shared, connection, approval, authorization)?;
    socket.io_memory = Some(
        reserve(
            shared,
            connection,
            ceiling
                .checked_mul(8)
                .and_then(|n| n.checked_add(128 * 1024))
                .ok_or(TransportError::Limit)?,
        )
        .await?,
    );
    // Keep reading Close/EOF while the serialized producer owner is blocked.
    // One envelope of queued bytes plus parser/copy working memory has a
    // separate per-view reservation; the fixed channel bounds tiny frames too.
    let ingress_limit = ceiling.checked_add(32 * 256).ok_or(TransportError::Limit)?;
    socket.ingress_memory = Some(
        reserve(
            shared,
            connection,
            ceiling
                .checked_mul(3)
                .and_then(|n| n.checked_add(64 * 1024))
                .ok_or(TransportError::Limit)?,
        )
        .await?,
    );
    let (mut sink, reader) = (&mut socket.socket).split();
    let (incoming, mut messages) = mpsc::channel(32);
    let read = read_ingress(reader, incoming, MemoryBudget::new(ingress_limit));
    let run = run_protocol(
        &mut sink,
        &mut messages,
        shared,
        connection,
        ceiling,
        approval,
        authorization,
        recording,
    );
    tokio::pin!(read, run);
    // This is one connection task, with no detached reader or producer task.
    // Cancelling the losing future releases its queue and exact request body;
    // the connection guard invalidates the native owner independently.
    tokio::select! {
        biased;
        result = &mut read => result,
        result = &mut run => result,
    }
}

struct IngressFrame {
    bytes: Zeroizing<Vec<u8>>,
    _memory: MemoryReservation,
}
async fn read_ingress<R>(
    mut reader: R,
    incoming: mpsc::Sender<IngressFrame>,
    budget: MemoryBudget,
) -> Result<(), TransportError>
where
    R: Stream<Item = Result<WebSocketMessage, WebSocketError>> + Unpin,
{
    let mut batch = 0;
    loop {
        if batch == 8 {
            // Buffered control frames must not starve the producer or its
            // deadlines simply because this reader remains immediately ready.
            tokio::task::yield_now().await;
            batch = 0;
        }
        batch += 1;
        match reader
            .next()
            .await
            .ok_or(TransportError::Closed)?
            .map_err(|_| TransportError::WebSocket)?
        {
            WebSocketMessage::Binary(bytes) => {
                let memory = budget
                    .reserve(bytes.len().checked_add(256).ok_or(TransportError::Limit)?)
                    .map_err(|_| TransportError::Limit)?;
                let frame = IngressFrame {
                    bytes: Zeroizing::new(bytes.to_vec()),
                    _memory: memory,
                };
                // Never park on a full normal queue: that would hide a Close
                // behind queued work. Overflow closes this exact connection.
                incoming
                    .try_send(frame)
                    .map_err(|_| TransportError::Limit)?;
            }
            WebSocketMessage::Close(_) => return Err(TransportError::Closed),
            WebSocketMessage::Ping(_) | WebSocketMessage::Pong(_) => {}
            _ => return Err(TransportError::Protocol),
        }
    }
}
#[allow(clippy::too_many_arguments)]
async fn run_protocol<S>(
    socket: &mut S,
    incoming: &mut mpsc::Receiver<IngressFrame>,
    shared: &Shared,
    connection: &Connection,
    ceiling: usize,
    approval: [u8; 32],
    authorization: &rui_protocol_core::session::VerifiedAuthorization,
    recording: Option<RecordingSession>,
) -> Result<(), TransportError>
where
    S: Sink<WebSocketMessage, Error = WebSocketError> + Unpin,
{
    let session = connection.session();
    let binding = Some((session.session_id, session.epoch, session.view_id));
    let timeout = shared.config.send_timeout;
    let mut delivery =
        Delivery::new(reserve(shared, connection, Delivery::reservation_bytes()).await?);
    let hello = p::ServerHello {
        selected_version: session.version,
        selected_profiles: session.profiles.iter().cloned().collect(),
        negotiated_limits: Some(codec::wire_limits(&session.limits)),
        permissions: Some(p::Permissions {
            can_view: session.permissions.can_view,
            can_control: session.permissions.can_control,
            actions: session.permissions.actions.iter().cloned().collect(),
        }),
        principal_id: {
            use sha2::{Digest, Sha256};
            Sha256::digest(session.client.principal.as_bytes())[..16].to_vec()
        },
    };
    send_recorded(
        recording.as_ref(),
        socket,
        codec::wrap(p::Kind::ServerHello, &hello, binding),
        ceiling,
        timeout,
        shared,
        connection,
        approval,
        authorization,
    )
    .await?;
    let mut opened = false;
    let mut queued = VecDeque::new();
    let chunk = session
        .limits
        .chunk_bytes
        .min(ceiling.saturating_sub(256))
        .min(session.limits.outbound_queue_bytes.saturating_sub(512) / 4);
    if chunk == 0 {
        return Err(TransportError::Limit);
    }
    let mut heartbeat = tokio::time::interval(Duration::from_millis(2));
    loop {
        tokio::select! {
            raw=incoming.recv()=>{
                let frame=raw.ok_or(TransportError::Closed)?;let raw=&frame.bytes;let packet=codec::packet(raw,ceiling,binding)?;let kind=p::Kind::try_from(packet.kind).map_err(|_|TransportError::Protocol)?;
                approved(shared,connection,approval,authorization)?;
                if kind==p::Kind::OpenView{
                    if opened{return Err(TransportError::Protocol);}let view:p::OpenView=codec::decode(&packet.payload,ceiling)?;
                    let received=request(shared,connection,Operation::Open{selection:view.selection,mirror_canvas_policy:view.mirror_canvas_policy}).await?;
                    let Reply::Opened{description,checkpoint}=received.value()else{return Err(TransportError::Protocol);};opened=true;
                    let message=p::ViewOpened{view_id:session.view_id.to_vec(),canvas_width:description.width,canvas_height:description.height,coverage:description.coverage.clone(),granted_input_modes:description.input_modes.clone(),input_devices:description.input_devices.iter().map(|d|p::InputDeviceGrant{seat_id:d.seat_id.clone(),device_id:d.device_id.clone(),key_namespace:d.key_namespace.clone(),input_mode:d.input_mode.clone(),text_owner:d.text_owner.clone()}).collect()};
                    let checkpoint=checkpoint.clone();drop(received);record_incoming(recording.as_ref(),kind,raw)?;
                    send_recorded(recording.as_ref(),socket,codec::wrap(p::Kind::ViewOpened,&message,binding),ceiling,timeout,shared,connection,approval,authorization).await?;
                    match &session.resume{
                        ResumeDisposition::Accepted{from_revision,through_revision,transactions,requires_resources}=>{
                            send_recorded(recording.as_ref(),socket,codec::wrap(p::Kind::ResumeAccepted,&p::ResumeAccepted{from_revision:*from_revision,through_revision:*through_revision,requires_resources:*requires_resources},binding),ceiling,timeout,shared,connection,approval,authorization).await?;
                        if from_revision==through_revision{let keeper=connection.resume_keeper().cloned().ok_or(TransportError::Protocol)?;source_scope(&keeper,session)?;delivery.seed_resumed(*from_revision,keeper);}
                            queued.extend(transactions.iter().cloned());
                        }
                        disposition=>{
                            if let ResumeDisposition::ResetRequired(reason)=disposition{send_recorded(recording.as_ref(),socket,codec::wrap(p::Kind::ResetRequired,&p::ResetRequired{reason:format!("{reason:?}")},binding),ceiling,timeout,shared,connection,approval,authorization).await?;}
                            queued.push_back(checkpoint.ok_or(TransportError::Protocol)?);
                        }
                    }
                }else{
                    if !opened{return Err(TransportError::Protocol);}
                    if matches!(kind,p::Kind::NeedResources|p::Kind::ResourceAvailable|p::Kind::ResourceEvicted){
                        let ids:p::ResourceIds=codec::decode(&packet.payload,ceiling)?;
                        let result=match kind{
                            p::Kind::NeedResources=>delivery.need(shared.port.as_ref(),connection,ids,chunk,shared.now()).await,
                            p::Kind::ResourceAvailable=>{request(shared,connection,Operation::Authorize).await?;delivery.available(ids)},
                            p::Kind::ResourceEvicted=>delivery.evicted(shared.port.as_ref(),connection,&ids,shared.now()).await.map(|_|()),
                            _=>unreachable!(),
                        };
                        match result{Ok(())=>record_incoming(recording.as_ref(),kind,raw)?,Err(TransportError::Authority) if kind==p::Kind::NeedResources=>send_recorded(recording.as_ref(),socket,codec::wrap(p::Kind::Error,&p::Error{code:"AUTHORITY_REJECTED".into()},binding),ceiling,timeout,shared,connection,approval,authorization).await?,Err(error)=>return Err(error)}
                        continue;
                    }
                    if kind==p::Kind::AppliedAck{
                        let ack:p::RevisionAck=codec::decode(&packet.payload,ceiling)?;delivery.applied(ack.revision)?;
                        request(shared,connection,Operation::Ack(Acknowledgement::Applied{revision:ack.revision,witness:delivery.applied_frame(ack.revision)?.clone()})).await?;
                        record_incoming(recording.as_ref(),kind,raw)?;continue;
                    }
                    let response=handle(shared,connection,kind,&packet.payload).await;
                    if response.is_ok(){record_incoming(recording.as_ref(),kind,raw)?;}
                    match response{Ok(Some((kind,bytes)))=>send_recorded(recording.as_ref(),socket,codec::wrap_bytes(kind,bytes,binding),ceiling,timeout,shared,connection,approval,authorization).await?,Ok(None)=>{},Err(TransportError::Authority)=>send_recorded(recording.as_ref(),socket,codec::wrap(p::Kind::Error,&p::Error{code:"AUTHORITY_REJECTED".into()},binding),ceiling,timeout,shared,connection,approval,authorization).await?,Err(error)=>return Err(error)}
                }
            }
            _=heartbeat.tick()=>{
                approved(shared,connection,approval,authorization)?;
                if delivery.started.is_some_and(|started|started.elapsed()>shared.config.publication_timeout){return Err(TransportError::Deadline);}
                let may_take=opened&&delivery.can_start()&&queued.is_empty();
                let received=request(shared,connection,Operation::Pump{take_publication:may_take}).await?;
                let Reply::Pump{publication,revoked}=received.value()else{return Err(TransportError::Protocol);};let publication=publication.clone();let revoked=revoked.clone();drop(received);
                if let Some(notice)=revoked{send_recorded(recording.as_ref(),socket,codec::wrap(p::Kind::ControlRevoked,&p::ControlRevoked{lease_id:notice.lease.to_vec(),generation:notice.generation,reason:notice.reason},binding),ceiling,timeout,shared,connection,approval,authorization).await?;}
                if !opened{continue;}
            if delivery.can_start(){if let Some(frame)=queued.pop_front().or(publication){source_scope(&frame,session)?;delivery.install(Outgoing::new(frame,chunk,session.limits.transaction_bytes.max(session.limits.snapshot_bytes))?);}}
                if let Some(packet)=delivery.next(binding)?{send_recorded(recording.as_ref(),socket,packet,ceiling,timeout,shared,connection,approval,authorization).await?;}
            }
        }
    }
}
fn record_incoming(
    recording: Option<&RecordingSession>,
    kind: p::Kind,
    raw: &[u8],
) -> Result<(), TransportError> {
    if let Some(recording) = recording {
        recording.record(Direction::ClientToHost, kind, raw)?;
    }
    Ok(())
}
fn approved(
    shared: &Shared,
    connection: &Connection,
    approval: [u8; 32],
    authorization: &rui_protocol_core::session::VerifiedAuthorization,
) -> Result<(), TransportError> {
    if !connection.authorized()
        || !shared
            .pairing
            .lock()
            .map_err(|_| TransportError::Authentication)?
            .remains_authorized(
                approval,
                connection.session().client.client_instance_id,
                authorization,
            )
    {
        return Err(TransportError::Authentication);
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)] // Keep the issued scope and approval explicit at every write.
async fn send_recorded<S>(
    recording: Option<&RecordingSession>,
    socket: &mut S,
    mut packet: p::Packet,
    ceiling: usize,
    timeout: Duration,
    shared: &Shared,
    connection: &Connection,
    approval: [u8; 32],
    authorization: &rui_protocol_core::session::VerifiedAuthorization,
) -> Result<(), TransportError>
where
    S: Sink<WebSocketMessage, Error = WebSocketError> + Unpin,
{
    approved(shared, connection, approval, authorization)?;
    // Approval lifetime alone does not establish the current incarnation. The
    // producer validates the issued capability before every attempted write.
    request(shared, connection, Operation::Authorize).await?;
    if packet.encoded_len() > ceiling {
        return Err(TransportError::Limit);
    }
    let kind = p::Kind::try_from(packet.kind).map_err(|_| TransportError::Protocol)?;
    let bytes = packet.encode_to_vec();
    packet.payload.zeroize();
    if let Some(recording) = recording {
        recording.record(Direction::HostToClient, kind, &bytes)?;
    }
    approved(shared, connection, approval, authorization)?;
    websocket::send_bytes(socket, bytes, timeout).await
}
async fn handle(
    shared: &Shared,
    connection: &Connection,
    kind: p::Kind,
    raw: &[u8],
) -> Result<Option<(p::Kind, Vec<u8>)>, TransportError> {
    let ceiling = connection.session().limits.envelope_bytes;
    let operation = match kind {
        p::Kind::SemanticReady => {
            let ack: p::RevisionAck = codec::decode(raw, ceiling)?;
            Operation::Ack(Acknowledgement::SemanticReady {
                revision: ack.revision,
            })
        }
        p::Kind::PresentedAck => {
            let ack: p::RevisionAck = codec::decode(raw, ceiling)?;
            Operation::Ack(Acknowledgement::Presented {
                revision: ack.revision,
            })
        }
        p::Kind::AcquireControl => {
            let req: p::AcquireControl = codec::decode(raw, ceiling)?;
            Operation::Control(Control::Acquire {
                request: codec::id(&req.request_id)?,
            })
        }
        p::Kind::RenewControl => {
            let req: p::RenewControl = codec::decode(raw, ceiling)?;
            Operation::Control(Control::Renew {
                request: codec::id(&req.request_id)?,
                lease: codec::id(&req.lease_id)?,
                generation: req.generation,
            })
        }
        p::Kind::ReleaseControl => {
            let req: p::ReleaseControl = codec::decode(raw, ceiling)?;
            Operation::Control(Control::Release {
                request: codec::id(&req.request_id)?,
                lease: codec::id(&req.lease_id)?,
                generation: req.generation,
            })
        }
        p::Kind::SemanticCommand
        | p::Kind::RawInputBatch
        | p::Kind::TextComposition
        | p::Kind::ResizeRequest => {
            use rui_protocol_core::commands::CommandFamily;
            let family = match kind {
                p::Kind::SemanticCommand => CommandFamily::Semantic,
                p::Kind::RawInputBatch => CommandFamily::RawInput,
                p::Kind::TextComposition => CommandFamily::TextComposition,
                p::Kind::ResizeRequest => CommandFamily::Resize,
                _ => unreachable!(),
            };
            rui_protocol_core::commands::decode_family(family, raw)
                .map_err(|_| TransportError::Protocol)?;
            Operation::Command {
                family,
                bytes: zeroize::Zeroizing::new(raw.to_vec()),
            }
        }
        p::Kind::GetCommandResult => {
            let req: p::GetCommandResult = codec::decode(raw, ceiling)?;
            Operation::Result {
                sequence: req.command_seq,
            }
        }
        p::Kind::CloseView => return Err(TransportError::Closed),
        p::Kind::Ping => {
            let ping: p::Ping = codec::decode(raw, ceiling)?;
            request(shared, connection, Operation::Authorize).await?;
            return Ok(Some((p::Kind::Pong, ping.encode_to_vec())));
        }
        _ => return Err(TransportError::Protocol),
    };
    let received = request(shared, connection, operation).await?;
    match received.value() {
        Reply::Done => Ok(None),
        Reply::Lease(lease) => {
            let request_id = if kind == p::Kind::AcquireControl {
                codec::decode::<p::AcquireControl>(raw, ceiling)?.request_id
            } else {
                codec::decode::<p::RenewControl>(raw, ceiling)?.request_id
            };
            Ok(Some((
                p::Kind::ControlGranted,
                p::ControlGranted {
                    lease_id: lease.lease_id.to_vec(),
                    generation: lease.generation,
                    expires_at_ms: lease.expires_at_ms,
                    allowed_actions: lease.allowed_actions.iter().cloned().collect(),
                    request_id,
                }
                .encode_to_vec(),
            )))
        }
        Reply::Command(result) => {
            let seq = if kind == p::Kind::GetCommandResult {
                codec::decode::<p::GetCommandResult>(raw, ceiling)?.command_seq
            } else {
                use rui_protocol_core::commands::CommandFamily;
                let family = match kind {
                    p::Kind::SemanticCommand => CommandFamily::Semantic,
                    p::Kind::RawInputBatch => CommandFamily::RawInput,
                    p::Kind::TextComposition => CommandFamily::TextComposition,
                    p::Kind::ResizeRequest => CommandFamily::Resize,
                    _ => return Err(TransportError::Protocol),
                };
                rui_protocol_core::commands::decode_family(family, raw)
                    .map_err(|_| TransportError::Protocol)?
                    .header()
                    .command_seq
            };
            Ok(Some((
                p::Kind::CommandResult,
                codec::command_result(seq, result.clone()).encode_to_vec(),
            )))
        }
        _ => Err(TransportError::Protocol),
    }
}

pub(crate) struct Outgoing {
    pub frame: SharedPayload,
    payload: std::ops::Range<usize>,
    pub begin: p::PublicationBegin,
    kind: p::Kind,
    pub stage: u32,
    index: u32,
    chunk: usize,
}
impl Outgoing {
    pub(crate) fn new(
        frame: SharedPayload,
        chunk: usize,
        maximum: usize,
    ) -> Result<Self, TransportError> {
        // Source envelopes are host-generated, bounded and contain one payload.
        // Keep the original SharedPayload and borrow slices rather than copying
        // a complete snapshot into another unaccounted queue.
        let (payload, core_kind) = source_envelope_header(&frame)?;
        if payload.len() > maximum || chunk == 0 {
            return Err(TransportError::Limit);
        }
        let (base, revision, count) = publication_header(&frame[payload.clone()], core_kind)?;
        if frame
            .publication_revision()
            .is_some_and(|stamped| stamped != revision)
        {
            return Err(TransportError::Protocol);
        }
        let kind = match core_kind {
            1 => p::Kind::SnapshotBegin,
            2 => p::Kind::TransactionBegin,
            _ => return Err(TransportError::Protocol),
        };
        let mut publication_id = [0; 16];
        getrandom::getrandom(&mut publication_id).map_err(|_| TransportError::Entropy)?;
        let begin = p::PublicationBegin {
            publication_id: publication_id.to_vec(),
            base_revision: base,
            revision,
            total_bytes: payload.len() as u64,
            chunk_count: u32::try_from(payload.len().div_ceil(chunk))
                .map_err(|_| TransportError::Limit)?,
            item_count: u32::try_from(count).map_err(|_| TransportError::Limit)?,
            digest: publication_digest(&frame[payload.clone()]).to_vec(),
        };
        Ok(Self {
            frame,
            payload,
            begin,
            kind,
            stage: 0,
            index: 0,
            chunk,
        })
    }
    pub fn next(
        &mut self,
        binding: Option<(Id, Id, Id)>,
    ) -> Result<Option<p::Packet>, TransportError> {
        if self.stage == 0 {
            self.stage = 1;
            return Ok(Some(codec::wrap(self.kind, &self.begin, binding)));
        }
        if self.index < self.begin.chunk_count {
            let start = self.payload.start + self.index as usize * self.chunk;
            let end = (start + self.chunk).min(self.payload.end);
            let chunk = p::PublicationChunk {
                publication_id: self.begin.publication_id.clone(),
                index: self.index,
                data: self.frame[start..end].to_vec(),
            };
            self.index += 1;
            return Ok(Some(codec::wrap(
                p::Kind::PublicationChunk,
                &chunk,
                binding,
            )));
        }
        if self.stage == 1 {
            self.stage = 2;
            let commit = p::PublicationCommit {
                publication_id: self.begin.publication_id.clone(),
                total_bytes: self.begin.total_bytes,
                chunk_count: self.begin.chunk_count,
                item_count: self.begin.item_count,
                digest: self.begin.digest.clone(),
            };
            return Ok(Some(codec::wrap(
                p::Kind::PublicationCommit,
                &commit,
                binding,
            )));
        }
        Ok(None)
    }
}
/// Borrowed top-level protobuf traversal: no payload/graph decode or allocation.
fn fields(
    raw: &[u8],
    mut visit: impl FnMut(u64, u64, &[u8], std::ops::Range<usize>) -> Result<(), TransportError>,
) -> Result<(), TransportError> {
    fn v(raw: &[u8], cursor: &mut usize) -> Result<u64, TransportError> {
        let mut value = 0;
        for shift in (0..70).step_by(7) {
            let byte = *raw.get(*cursor).ok_or(TransportError::Protocol)?;
            *cursor += 1;
            if shift == 63 && byte > 1 {
                return Err(TransportError::Protocol);
            }
            value |= u64::from(byte & 127) << shift;
            if byte & 128 == 0 {
                return Ok(value);
            }
        }
        Err(TransportError::Protocol)
    }
    let mut cursor = 0;
    while cursor < raw.len() {
        let tag = v(raw, &mut cursor)?;
        if tag >> 3 == 0 {
            return Err(TransportError::Protocol);
        }
        let mut scalar = 0;
        let length = match tag & 7 {
            0 => {
                scalar = v(raw, &mut cursor)?;
                0
            }
            1 => 8,
            2 => usize::try_from(v(raw, &mut cursor)?).map_err(|_| TransportError::Limit)?,
            5 => 4,
            _ => return Err(TransportError::Protocol),
        };
        let end = cursor
            .checked_add(length)
            .filter(|n| *n <= raw.len())
            .ok_or(TransportError::Protocol)?;
        visit(tag, scalar, &raw[cursor..end], cursor..end)?;
        cursor = end;
    }
    Ok(())
}
fn source_envelope_header(raw: &[u8]) -> Result<(std::ops::Range<usize>, u64), TransportError> {
    let mut payload = None;
    let mut kind = None;
    fields(raw, |tag, value, _, range| {
        if tag == 40 && kind.replace(value).is_some() {
            return Err(TransportError::Protocol);
        }
        if tag == 50 && payload.replace(range).is_some() {
            return Err(TransportError::Protocol);
        }
        Ok(())
    })?;
    Ok((
        payload.ok_or(TransportError::Protocol)?,
        kind.ok_or(TransportError::Protocol)?,
    ))
}
/// Validate the trusted producer's outer scope before stripping its envelope.
/// Otherwise a wrong-view source body could acquire this socket's binding.
fn source_scope(
    raw: &[u8],
    session: &rui_protocol_core::session::NegotiatedSession,
) -> Result<(), TransportError> {
    let mut version = None;
    let mut ids: [Option<[u8; 16]>; 3] = [None, None, None];
    let mut required = 0usize;
    fields(raw, |tag, value, data, _| {
        match tag {
            8 => {
                if version.replace(value).is_some() {
                    return Err(TransportError::Protocol);
                }
            }
            18 | 26 | 34 => {
                let index = (tag as usize - 18) / 8;
                if ids[index]
                    .replace(data.try_into().map_err(|_| TransportError::Binding)?)
                    .is_some()
                {
                    return Err(TransportError::Protocol);
                }
            }
            40 | 50 => {}
            58 => {
                required = required
                    .checked_add(1)
                    .filter(|n| *n <= 128)
                    .ok_or(TransportError::Limit)?;
                if data.len() > 256
                    || !session
                        .profiles
                        .contains(std::str::from_utf8(data).map_err(|_| TransportError::Protocol)?)
                {
                    return Err(TransportError::Protocol);
                }
            }
            _ if tag >> 3 >= 128 => {}
            _ => return Err(TransportError::Protocol),
        }
        Ok(())
    })?;
    if version != Some(u64::from(session.version))
        || ids[0] != Some(session.session_id)
        || ids[1] != Some(session.epoch)
        || ids[2] != Some(session.view_id)
    {
        return Err(TransportError::Binding);
    }
    Ok(())
}
fn publication_header(raw: &[u8], kind: u64) -> Result<(u64, u64, usize), TransportError> {
    if kind != 1 && kind != 2 {
        return Err(TransportError::Protocol);
    }
    let mut first = None;
    let mut second = None;
    let mut count = 0usize;
    fields(raw, |tag, value, _, _| {
        match (kind, tag) {
            (_, 8) => {
                if first.replace(value).is_some() {
                    return Err(TransportError::Protocol);
                }
            }
            (2, 16) => {
                if second.replace(value).is_some() {
                    return Err(TransportError::Protocol);
                }
            }
            (1, 18) | (2, 26) => {
                count = count.checked_add(1).ok_or(TransportError::Limit)?;
            }
            _ => {}
        }
        Ok(())
    })?;
    let first = first.ok_or(TransportError::Protocol)?;
    if kind == 1 {
        Ok((0, first, count))
    } else {
        Ok((first, second.ok_or(TransportError::Protocol)?, count))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rui_host_broker::{HostConfig, COUNTER_ID};
    use rui_protocol_core::session::{
        ClientHello, Permissions, ReceiverLimits, SessionConfig, VerifiedAuthorization,
    };
    use std::sync::atomic::Ordering;
    async fn setup(
        cleanup: Arc<dyn Fn(InputCleanup) + Send + Sync>,
    ) -> (
        Arc<Shared>,
        Connection,
        Connection,
        Arc<Mutex<ControlledHost>>,
    ) {
        let auth = VerifiedAuthorization {
            principal: "operator".into(),
            policy_version: 1,
            permissions: Permissions {
                can_view: true,
                can_control: true,
                actions: ["Activate".into()].into_iter().collect(),
            },
        };
        let host = Arc::new(Mutex::new(
            ControlledHost::new(
                HostConfig {
                    max_text_bytes: 64,
                    ..Default::default()
                },
                String::new(),
            )
            .unwrap(),
        ));
        let port = producer::ControlledPort::new(host.clone(), cleanup.clone(), 2).unwrap();
        let mut pairing = PairingAuthority::new(2).unwrap();
        let mut connections = Vec::new();
        for id in [3, 4] {
            let req = pairing.request_local_pairing([id; 16]).unwrap();
            let credential = pairing.approve_local(req.request_id, auth.clone()).unwrap();
            let lifetime = pairing
                .approval_lifetime(credential.expose_for_tls_authentication(), [id; 16])
                .unwrap();
            let received = port
                .transact(Request::Connect {
                    authorization: auth.clone(),
                    hello: Box::new(ClientHello {
                        client_instance_id: [id; 16],
                        supported_versions: [1].into_iter().collect(),
                        supported_profiles: SessionConfig::default().supported_profiles,
                        required_profiles: SessionConfig::default().required_profiles,
                        receiver_limits: ReceiverLimits::default(),
                        resume: None,
                    }),
                    lifetime,
                    fresh_view: false,
                    now: 0,
                })
                .await
                .unwrap();
            let Reply::Connected(connection) = received.value() else {
                panic!()
            };
            let connection = connection.clone();
            drop(received);
            let received = port
                .transact(Request::Scoped {
                    connection: connection.clone(),
                    operation: Operation::Open {
                        selection: "fixture".into(),
                        mirror_canvas_policy: "host-canvas".into(),
                    },
                    now: 0,
                })
                .await
                .unwrap();
            let Reply::Opened {
                checkpoint: Some(frame),
                ..
            } = received.value()
            else {
                panic!()
            };
            let frame = frame.clone();
            drop(received);
            port.transact(Request::Scoped {
                connection: connection.clone(),
                operation: Operation::Ack(Acknowledgement::Applied {
                    revision: 1,
                    witness: frame,
                }),
                now: 0,
            })
            .await
            .unwrap();
            port.transact(Request::Scoped {
                connection: connection.clone(),
                operation: Operation::Ack(Acknowledgement::Presented { revision: 1 }),
                now: 0,
            })
            .await
            .unwrap();
            connections.push(connection);
        }
        let first = connections.remove(0);
        let second = connections.remove(0);
        port.transact(Request::Scoped {
            connection: first.clone(),
            operation: Operation::Control(Control::Acquire { request: [9; 16] }),
            now: 0,
        })
        .await
        .unwrap();
        (
            Arc::new(Shared {
                port: Arc::new(port),
                pairing: Arc::new(StdMutex::new(pairing)),
                active: StdMutex::new(BTreeSet::new()),
                start: Instant::now(),
                config: ServerConfig {
                    cleanup,
                    ..Default::default()
                },
            }),
            first,
            second,
            host,
        )
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn cleanup_completes_before_another_connection_can_acquire() {
        let (entered_tx, entered_rx) = oneshot::channel();
        let entered = StdMutex::new(Some(entered_tx));
        let (release, wait) = std::sync::mpsc::channel();
        let wait = StdMutex::new(wait);
        let callback = Arc::new(move |_: InputCleanup| {
            entered.lock().unwrap().take().unwrap().send(()).unwrap();
            // Let the test runtime schedule the competing socket while this
            // deliberately synchronous trusted cleanup owns the host lock.
            tokio::task::block_in_place(|| {
                wait.lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap();
            });
        });
        let (shared, _, second, host) = setup(callback).await;
        host.lock().await.local_activity(0);
        let draining = {
            let shared = shared.clone();
            tokio::spawn(async move {
                shared.port.flush_lifecycle(0).await.unwrap();
            })
        };
        entered_rx.await.unwrap();
        let mut acquisition = {
            let shared = shared.clone();
            tokio::spawn(async move {
                handle(
                    &shared,
                    &second,
                    p::Kind::AcquireControl,
                    &p::AcquireControl {
                        request_id: vec![7; 16],
                    }
                    .encode_to_vec(),
                )
                .await
            })
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut acquisition)
                .await
                .is_err()
        );
        release.send(()).unwrap();
        draining.await.unwrap();
        assert!(acquisition.await.unwrap().unwrap().is_some());
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cleanup_panic_keeps_authority_gate_closed() {
        let (shared, _, second, host) = setup(Arc::new(|_| panic!("trusted cleanup failed"))).await;
        host.lock().await.local_activity(0);
        let task = {
            let shared = shared.clone();
            tokio::spawn(async move { shared.port.flush_lifecycle(0).await })
        };
        assert_eq!(task.await.unwrap(), Err(TransportError::Authority));
        assert_eq!(
            handle(
                &shared,
                &second,
                p::Kind::AcquireControl,
                &p::AcquireControl {
                    request_id: vec![7; 16]
                }
                .encode_to_vec(),
            )
            .await,
            Err(TransportError::Authority)
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn revocation_while_waiting_for_host_lock_prevents_ledger_admission() {
        let (shared, first, _, host) = setup(Arc::new(|_| {})).await;
        let locked = host.lock().await;
        let command = rui_protocol_core::wire::proto::SemanticCommand {
            epoch: first.session().epoch.to_vec(),
            view_id: first.session().view_id.to_vec(),
            client_instance_id: first.session().client.client_instance_id.to_vec(),
            command_seq: 1,
            target_id: COUNTER_ID,
            control_lease_id: vec![9; 16],
            control_generation: 1,
            action_id: "Activate".into(),
            action_schema_version: 1,
            observed_view_revision: 1,
            arguments: Some(
                rui_protocol_core::wire::proto::semantic_command::Arguments::NoArguments(
                    rui_protocol_core::wire::proto::EmptyArguments {},
                ),
            ),
            ..Default::default()
        }
        .encode_to_vec();
        let task = {
            let shared = shared.clone();
            tokio::spawn(async move {
                handle(&shared, &first, p::Kind::SemanticCommand, &command).await
            })
        };
        tokio::task::yield_now().await;
        shared.pairing.lock().unwrap().revoke_local([3; 16]);
        drop(locked);
        assert_eq!(task.await.unwrap(), Err(TransportError::Authentication));
        assert_eq!(host.lock().await.diagnostics().consumed_commands, 0);
    }
    #[tokio::test]
    async fn queued_cleanup_is_drained_by_acquire_before_grant() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = calls.clone();
        let (shared, _, second, host) = setup(Arc::new(move |_| {
            observed.fetch_add(1, Ordering::AcqRel);
        }))
        .await;
        host.lock().await.local_activity(0);
        assert!(handle(
            &shared,
            &second,
            p::Kind::AcquireControl,
            &p::AcquireControl {
                request_id: vec![7; 16]
            }
            .encode_to_vec(),
        )
        .await
        .unwrap()
        .is_some());
        assert_eq!(calls.load(Ordering::Acquire), 1);
    }
    #[test]
    fn publication_metadata_borrows_and_does_not_decode_graph() {
        // Malformed nested record proves this path inspects headers only.
        let snapshot = [8, 1, 18, 1, 0];
        let envelope = [40, 1, 50, 5, 8, 1, 18, 1, 0];
        assert!(rui_protocol_core::wire::proto::Snapshot::decode(&snapshot[..]).is_err());
        let (range, kind) = source_envelope_header(&envelope).unwrap();
        assert_eq!(&envelope[range.clone()], &snapshot);
        assert_eq!(
            publication_header(&envelope[range], kind).unwrap(),
            (0, 1, 1)
        );
        assert!(source_envelope_header(&[40, 1, 50, 0, 50, 0]).is_err());
    }
    #[tokio::test]
    async fn producer_envelope_must_match_issued_scope_and_required_profiles() {
        use rui_protocol_core::wire::proto as core;
        let (shared, first, _, _) = setup(Arc::new(|_| {})).await;
        let session = first.session();
        let envelope = core::Envelope {
            protocol_version: session.version,
            session_id: session.session_id.to_vec(),
            epoch: session.epoch.to_vec(),
            view_id: session.view_id.to_vec(),
            kind: core::MessageKind::Snapshot as i32,
            payload: vec![8, 1],
            required_features: session.profiles.iter().take(1).cloned().collect(),
        };
        let bytes = envelope.encode_to_vec();
        source_scope(&bytes, session).unwrap();
        let mut wrong = envelope.clone();
        wrong.view_id[0] ^= 1;
        assert_eq!(
            source_scope(&wrong.encode_to_vec(), session),
            Err(TransportError::Binding)
        );
        wrong = envelope.clone();
        wrong.protocol_version += 1;
        assert_eq!(
            source_scope(&wrong.encode_to_vec(), session),
            Err(TransportError::Binding)
        );
        wrong = envelope.clone();
        wrong.required_features.push("unknown.required.v1".into());
        assert_eq!(
            source_scope(&wrong.encode_to_vec(), session),
            Err(TransportError::Protocol)
        );
        let mut duplicate = bytes.clone();
        duplicate.extend([34, 16]);
        duplicate.extend(session.view_id);
        assert_eq!(
            source_scope(&duplicate, session),
            Err(TransportError::Protocol)
        );
        let mut required = bytes.clone();
        required.extend([64, 1]); // unknown required envelope field8
        assert_eq!(
            source_scope(&required, session),
            Err(TransportError::Protocol)
        );
        let mut optional = bytes;
        optional.extend([128, 8, 1]); // optional field128 metadata
        source_scope(&optional, session).unwrap();
        shared.port.shutdown();
        shared.port.flush_lifecycle(0).await.unwrap();
    }
}

#[cfg(test)]
mod ingress_tests {
    use super::*;
    use futures_util::SinkExt;
    use std::task::Poll;
    use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};

    #[tokio::test]
    async fn ping_and_close_are_read_while_the_outgoing_sink_is_backpressured() {
        let (peer, stream) = tokio::io::duplex(64);
        let mut client = WebSocketStream::from_raw_socket(peer, Role::Client, None).await;
        let socket = WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
        let (mut sink, reader) = socket.split();
        let writer = sink.send(WebSocketMessage::Binary(vec![9; 1024].into()));
        tokio::pin!(writer);
        assert!(matches!(futures_util::poll!(&mut writer), Poll::Pending));
        // The peer never consumes the full server message. Its Ping makes the
        // server schedule an automatic Pong on that same blocked writer.
        client
            .send(WebSocketMessage::Ping(vec![1].into()))
            .await
            .unwrap();
        client.close(None).await.unwrap();
        let (incoming, _messages) = mpsc::channel(32);
        let budget = MemoryBudget::new(4096);
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(1),
                read_ingress(reader, incoming, budget.clone()),
            )
            .await
            .unwrap(),
            Err(TransportError::Closed),
        );
        assert_eq!(budget.used(), 0);
    }
}
