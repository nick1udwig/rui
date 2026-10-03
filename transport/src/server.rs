use crate::diagnostics::{Diagnostics, RecordingSession};
use crate::source_delivery::Delivery;
use crate::{
    codec, pairing::PairingAuthority, proto as p, websocket, HostIdentity, TransportError,
};
use prost::Message;
use rui_host_broker::ControlledHost;
use rui_protocol_core::commands::Id;
use rui_protocol_core::publication::publication_digest;
use rui_protocol_core::session::{
    InputCleanup, NegotiatedSession, ResumeDisposition, SharedPayload,
};
use rui_protocol_core::transcript::Direction;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::{Duration, Instant},
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{oneshot, Mutex, Semaphore},
    task::{JoinHandle, JoinSet},
};
use zeroize::Zeroize;

#[derive(Clone)]
pub struct ServerConfig {
    /// Opt-in trusted local diagnostics; exact bytes require explicit policy.
    pub diagnostics: Option<Arc<Diagnostics>>,
    pub max_connections: usize,
    pub max_message_bytes: usize,
    pub handshake_timeout: Duration,
    pub send_timeout: Duration,
    /// Whole publication/resource wait ceiling, including delayed APPLIED.
    pub publication_timeout: Duration,
    /// Trusted host adapter callback: execute all cleanup before another lease
    /// can be granted. The network never silently discards held-input cleanup.
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
    host: Arc<Mutex<ControlledHost>>,
    pairing: Arc<StdMutex<PairingAuthority>>,
    active: StdMutex<BTreeSet<Id>>,
    revoked: StdMutex<BTreeMap<Id, p::ControlRevoked>>,
    cleanup_busy: AtomicBool,
    start: Instant,
    config: ServerConfig,
}
impl Shared {
    fn now(&self) -> u64 {
        self.start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
    }
}
pub struct ServerHandle {
    pub address: SocketAddr,
    pub host: Arc<Mutex<ControlledHost>>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}
impl ServerHandle {
    pub async fn bind(
        address: SocketAddr,
        identity: HostIdentity,
        host: ControlledHost,
        pairing: Arc<StdMutex<PairingAuthority>>,
        config: ServerConfig,
    ) -> Result<Self, TransportError> {
        if config.max_connections == 0
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
        let host = Arc::new(Mutex::new(host));
        let shared = Arc::new(Shared {
            host: host.clone(),
            pairing,
            active: StdMutex::new(BTreeSet::new()),
            revoked: StdMutex::new(BTreeMap::new()),
            cleanup_busy: AtomicBool::new(false),
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
                    accepted=listener.accept()=>{let Ok((tcp,_))=accepted else {break;};let Ok(permit)=permits.clone().try_acquire_owned() else {drop(tcp);continue;};
                        let shared=shared.clone();let acceptor=acceptor.clone();tasks.spawn(async move {let _permit=permit;let _=connection(tcp,acceptor,shared).await;});}
                }
            }
            shared.host.lock().await.local_activity(shared.now());
            drain_cleanup(&shared).await;
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        });
        Ok(Self {
            address,
            host,
            stop: Some(stop),
            task: Some(task),
        })
    }
    pub async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}
impl Drop for ServerHandle {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
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
        if let Ok(mut revoked) = self.shared.revoked.lock() {
            revoked.remove(&self.id);
        }
    }
}
async fn drain_cleanup(shared: &Shared) {
    let mut host = shared.host.lock().await;
    drain_host_cleanup(shared, &mut host);
}
fn drain_host_cleanup(shared: &Shared, host: &mut ControlledHost) {
    if shared.cleanup_busy.load(Ordering::Acquire) {
        return;
    }
    while let Some(cleanup) = host.take_cleanup() {
        shared.cleanup_busy.store(true, Ordering::Release);
        let notice = p::ControlRevoked {
            lease_id: cleanup.lease.lease_id.to_vec(),
            generation: cleanup.lease.generation,
            reason: format!("{:?}", cleanup.reason),
        };
        let client = cleanup.lease.client.client_instance_id;
        (shared.config.cleanup)(cleanup);
        shared.cleanup_busy.store(false, Ordering::Release);
        if shared.active.lock().is_ok_and(|a| a.contains(&client)) {
            if let Ok(mut revoked) = shared.revoked.lock() {
                revoked.insert(client, notice);
            }
        }
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
    // The upgrade selected the parser bound before reading an envelope body.
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
    let (session, resume_keeper) = {
        let mut host = shared.host.lock().await;
        let pairing = shared
            .pairing
            .lock()
            .map_err(|_| TransportError::Authentication)?;
        if !pairing.remains_authorized(approval, client_id, &authorization) {
            return Err(TransportError::Authentication);
        }
        if fresh_view {
            host.retire_registered_view(
                &rui_protocol_core::commands::AuthenticatedClient {
                    principal: authorization.principal.clone(),
                    client_instance_id: client_id,
                },
                shared.now(),
            )
            .map_err(|_| TransportError::Authority)?;
        }
        let session = host
            .negotiate_with_lifetime(
                authorization.clone(),
                hello,
                Some(lifetime.clone()),
                shared.now(),
            )
            .map_err(|_| TransportError::Authority)?;
        // Capture the exact zero-delta keeper while negotiation and source
        // state share the host lock; later source changes queue normal deltas.
        let keeper = if matches!(&session.resume,ResumeDisposition::Accepted {from_revision,through_revision,..} if from_revision==through_revision)
        {
            match host.snapshot_publication(&session.client, session.view_id) {
                Ok(frame) => Some(frame),
                Err(_) => {
                    let _ = host.disconnect(&session.client, shared.now());
                    drop(pairing);
                    drain_host_cleanup(&shared, &mut host);
                    return Err(TransportError::Limit);
                }
            }
        } else {
            None
        };
        (session, keeper)
    };
    let client = session.client.clone();
    let recording = if let Some(owner) = &shared.config.diagnostics {
        let host = shared.host.lock().await;
        let memory = host
            .reserve_view_memory(&client, session.view_id, owner.reservation_bytes())
            .map_err(|_| TransportError::Limit);
        match memory.and_then(|memory| owner.open(&session, lifetime, memory)) {
            Ok(recording) => Some(recording),
            Err(error) => {
                drop(host);
                let _ = shared.host.lock().await.disconnect(&client, shared.now());
                drain_cleanup(&shared).await;
                return Err(error);
            }
        }
    } else {
        None
    };
    let result = run_authenticated(
        &mut socket,
        &shared,
        session,
        ceiling,
        approval,
        authorization,
        resume_keeper,
        recording,
    )
    .await;
    let _ = shared.host.lock().await.disconnect(&client, shared.now());
    drain_cleanup(&shared).await;
    result
}
#[allow(clippy::too_many_arguments)] // Authenticated ownership and resume keeper remain explicit.
async fn run_authenticated<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    shared: &Shared,
    session: NegotiatedSession,
    ceiling: usize,
    approval: [u8; 32],
    authorization: rui_protocol_core::session::VerifiedAuthorization,
    mut resume_keeper: Option<SharedPayload>,
    recording: Option<RecordingSession>,
) -> Result<(), TransportError> {
    if session.limits.envelope_bytes != ceiling {
        return Err(TransportError::Limit);
    }
    let binding = Some((session.session_id, session.epoch, session.view_id));
    let timeout = shared.config.send_timeout;
    // Lifetime headroom is acquired before a post-authenticated socket send
    // can retain encoded bytes. It also covers one bounded incoming decode.
    let (_io_memory, mut delivery) = {
        let host = shared.host.lock().await;
        let _pairing = approved(shared, &session, approval, &authorization)?;
        let memory = host
            .reserve_view_memory(
                &session.client,
                session.view_id,
                ceiling
                    .checked_mul(8)
                    .and_then(|n| n.checked_add(128 * 1024))
                    .ok_or(TransportError::Limit)?,
            )
            .map_err(|_| TransportError::Limit)?;
        (memory, Delivery::new(&host, &session)?)
    };
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
    )
    .await?;
    let mut opened = false;
    let mut queued = VecDeque::new();
    let chunk_size = session
        .limits
        .chunk_bytes
        .min(ceiling.saturating_sub(256))
        .min(session.limits.outbound_queue_bytes.saturating_sub(512) / 4);
    if chunk_size == 0 {
        return Err(TransportError::Limit);
    }
    let mut heartbeat = tokio::time::interval(Duration::from_millis(2));
    loop {
        tokio::select! {
            raw=websocket::receive(socket)=>{
                let raw=raw?;let packet=codec::packet(&raw,ceiling,binding)?;let kind=p::Kind::try_from(packet.kind).map_err(|_|TransportError::Protocol)?;
                if !shared.pairing.lock().map_err(|_|TransportError::Authentication)?.remains_authorized(approval,session.client.client_instance_id,&authorization) {return Err(TransportError::Authentication);}
                if kind==p::Kind::OpenView {
                    if opened {return Err(TransportError::Protocol);}let view:p::OpenView=codec::decode(&packet.payload,ceiling)?;
                    let semantic_only=session.profiles.contains("semantics.only.v1");
                    if view.selection!="fixture" || view.mirror_canvas_policy!=if semantic_only {"semantic-only"}else {"host-canvas"} {return Err(TransportError::Protocol);}opened=true;
                    let description={let host=shared.host.lock().await;let _pairing=approved(shared,&session,approval,&authorization)?;host.view_description(&session.client,session.view_id).map_err(|_|TransportError::Authority)?};
                    record_incoming(recording.as_ref(),kind,&raw)?;
                    let opened=p::ViewOpened {view_id:session.view_id.to_vec(),canvas_width:description.width,canvas_height:description.height,coverage:description.coverage,granted_input_modes:description.input_modes,input_devices:description.input_devices.into_iter().map(|d|p::InputDeviceGrant{seat_id:d.seat_id,device_id:d.device_id,key_namespace:d.key_namespace,input_mode:d.input_mode,text_owner:d.text_owner}).collect()};
                    send_recorded(recording.as_ref(), socket,codec::wrap(p::Kind::ViewOpened,&opened,binding),ceiling,timeout).await?;
                    match &session.resume {
                        ResumeDisposition::Accepted {from_revision,through_revision,transactions,requires_resources}=>{
                            let resumed=p::ResumeAccepted {from_revision:*from_revision,through_revision:*through_revision,requires_resources:*requires_resources};
                            send_recorded(recording.as_ref(), socket,codec::wrap(p::Kind::ResumeAccepted,&resumed,binding),ceiling,timeout).await?;
                            if from_revision==through_revision {
                                delivery.seed_resumed(*from_revision,resume_keeper.take().ok_or(TransportError::Protocol)?);
                            }
                            queued.extend(transactions.iter().cloned());
                        }
                        _=>{
                            if let ResumeDisposition::ResetRequired(reason)=&session.resume {send_recorded(recording.as_ref(), socket,codec::wrap(p::Kind::ResetRequired,&p::ResetRequired {reason:format!("{reason:?}")},binding),ceiling,timeout).await?;}
                            let mut host=shared.host.lock().await;let pairing=approved(shared,&session,approval,&authorization)?;
                            queued.push_back(host.checkpoint(&session.client,session.view_id).map_err(|_|TransportError::Authority)?);drop(pairing);
                        }
                    }
                }else {
                    if !opened {return Err(TransportError::Protocol);}
                    if matches!(kind,p::Kind::NeedResources|p::Kind::ResourceAvailable|p::Kind::ResourceEvicted) {
                        let ids:p::ResourceIds=codec::decode(&packet.payload,ceiling)?;
                        let rejected={
                            let mut host=shared.host.lock().await;
                            let _pairing=approved(shared,&session,approval,&authorization)?;
                            match kind {
                                p::Kind::NeedResources=>match delivery.need(&host,&session,ids,chunk_size) {
                                    Ok(())=>false,
                                    Err(TransportError::Authority)=>true,
                                    Err(error)=>return Err(error),
                                },
                                p::Kind::ResourceAvailable=>{delivery.available(ids)?;false},
                                p::Kind::ResourceEvicted=>{
                                    if delivery.evicted(&mut host,&session,&ids)? {
                                        host.mark_resources_missing(&session.client,session.view_id).map_err(|_|TransportError::Authority)?;
                                    }
                                    false
                                }
                                _=>unreachable!(),
                            }
                        };
                        if !rejected {record_incoming(recording.as_ref(),kind,&raw)?;}
                        if rejected {send_recorded(recording.as_ref(), socket,codec::wrap(p::Kind::Error,&p::Error {code:"AUTHORITY_REJECTED".into()},binding),ceiling,timeout).await?;}
                        continue;
                    }
                    if kind==p::Kind::AppliedAck {
                        let ack:p::RevisionAck=codec::decode(&packet.payload,ceiling)?;
                        let mut host=shared.host.lock().await;
                        let _pairing=approved(shared,&session,approval,&authorization)?;
                        delivery.applied(ack.revision)?;
                        let frame=delivery.applied_frame(ack.revision)?;
                        if frame.publication_revision().is_some() {
                            host.acknowledge_applied_publication(&session.client,session.view_id,ack.revision,frame)
                                .map_err(|_|TransportError::Authority)?;
                        }else {
                            host.acknowledge_applied(&session.client,session.view_id,ack.revision)
                                .map_err(|_|TransportError::Authority)?;
                        }
                        record_incoming(recording.as_ref(),kind,&raw)?;
                        continue;
                    }
                    let response=handle(shared,&session,kind,&packet.payload,approval,&authorization).await;
                    if response.is_ok() {record_incoming(recording.as_ref(),kind,&raw)?;}
                    match response {Ok(Some((kind,bytes)))=>send_recorded(recording.as_ref(), socket,codec::wrap_bytes(kind,bytes,binding),ceiling,timeout).await?,Ok(None)=>{},
                        Err(TransportError::Authority)=>send_recorded(recording.as_ref(), socket,codec::wrap(p::Kind::Error,&p::Error {code:"AUTHORITY_REJECTED".into()},binding),ceiling,timeout).await?,Err(error)=>return Err(error)}
                }
            }
            _=heartbeat.tick()=>{
                if !shared.pairing.lock().map_err(|_|TransportError::Authentication)?.remains_authorized(approval,session.client.client_instance_id,&authorization) {return Err(TransportError::Authentication);}
                shared.host.lock().await.tick(shared.now());drain_cleanup(shared).await;
                let notice=shared.revoked.lock().map_err(|_|TransportError::Authority)?.remove(&session.client.client_instance_id);
                if let Some(notice)=notice {send_recorded(recording.as_ref(), socket,codec::wrap(p::Kind::ControlRevoked,&notice,binding),ceiling,timeout).await?;}
                if !opened {continue;}
                if delivery.started.is_some_and(|started|started.elapsed()>shared.config.publication_timeout) {return Err(TransportError::Deadline);}
                if delivery.can_start() {
                    let frame={let mut host=shared.host.lock().await;let _pairing=approved(shared,&session,approval,&authorization)?;
                        if let Some(frame)=queued.pop_front() {Some(frame)}else {host.take_outbound(&session.client,session.view_id).map_err(|_|TransportError::Authority)?}};
                    if let Some(frame)=frame {delivery.install(Outgoing::new(frame,chunk_size,session.limits.transaction_bytes.max(session.limits.snapshot_bytes))?);}
                }
                let packet={let host=shared.host.lock().await;let _pairing=approved(shared,&session,approval,&authorization)?;
                    host.view_description(&session.client,session.view_id).map_err(|_|TransportError::Authority)?;delivery.next(binding)?};
                if let Some(packet)=packet {send_recorded(recording.as_ref(), socket,packet,ceiling,timeout).await?;}
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
async fn send_recorded<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    recording: Option<&RecordingSession>,
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    mut packet: p::Packet,
    ceiling: usize,
    timeout: Duration,
) -> Result<(), TransportError> {
    if packet.encoded_len() > ceiling {
        return Err(TransportError::Limit);
    }
    let kind = p::Kind::try_from(packet.kind).map_err(|_| TransportError::Protocol)?;
    let bytes = packet.encode_to_vec();
    packet.payload.zeroize();
    if let Some(recording) = recording {
        recording.record(Direction::HostToClient, kind, &bytes)?;
    }
    // Exact same bytes enter the maintained WebSocket/TLS sink. This is an
    // attempted send, not proof of remote receipt or presentation.
    websocket::send_bytes(socket, bytes, timeout).await
}
fn approved<'a>(
    shared: &'a Shared,
    session: &NegotiatedSession,
    approval: [u8; 32],
    authorization: &rui_protocol_core::session::VerifiedAuthorization,
) -> Result<std::sync::MutexGuard<'a, PairingAuthority>, TransportError> {
    let guard = shared
        .pairing
        .lock()
        .map_err(|_| TransportError::Authentication)?;
    if !guard.remains_authorized(approval, session.client.client_instance_id, authorization) {
        return Err(TransportError::Authentication);
    }
    Ok(guard)
}
async fn handle(
    shared: &Shared,
    session: &NegotiatedSession,
    kind: p::Kind,
    raw: &[u8],
    approval: [u8; 32],
    authorization: &rui_protocol_core::session::VerifiedAuthorization,
) -> Result<Option<(p::Kind, Vec<u8>)>, TransportError> {
    let ceiling = session.limits.envelope_bytes;
    let mut host = shared.host.lock().await;
    host.tick(shared.now());
    drain_host_cleanup(shared, &mut host);
    let _pairing = approved(shared, session, approval, authorization)?;
    let client = &session.client;
    let view = session.view_id;
    let now = shared.now();
    let granted =
        |lease: rui_protocol_core::session::ControlLease, request_id: Vec<u8>| p::ControlGranted {
            lease_id: lease.lease_id.to_vec(),
            generation: lease.generation,
            expires_at_ms: lease.expires_at_ms,
            allowed_actions: lease.allowed_actions.into_iter().collect(),
            request_id,
        };
    let result = match kind {
        p::Kind::AppliedAck => {
            let ack: p::RevisionAck = codec::decode(raw, ceiling)?;
            host.acknowledge_applied(client, view, ack.revision)
                .map_err(|_| TransportError::Authority)?;
            None
        }
        p::Kind::SemanticReady => {
            let ack: p::RevisionAck = codec::decode(raw, ceiling)?;
            host.acknowledge_semantic_ready(client, view, ack.revision)
                .map_err(|_| TransportError::Authority)?;
            None
        }
        p::Kind::PresentedAck => {
            let ack: p::RevisionAck = codec::decode(raw, ceiling)?;
            host.acknowledge_presented(client, view, ack.revision)
                .map_err(|_| TransportError::Authority)?;
            None
        }
        p::Kind::AcquireControl => {
            if shared.cleanup_busy.load(Ordering::Acquire) {
                return Err(TransportError::Authority);
            }
            let request: p::AcquireControl = codec::decode(raw, ceiling)?;
            let lease = host
                .acquire_control(client, view, codec::id(&request.request_id)?, now)
                .map_err(|_| TransportError::Authority)?;
            Some((
                p::Kind::ControlGranted,
                granted(lease, request.request_id).encode_to_vec(),
            ))
        }
        p::Kind::RenewControl => {
            let request: p::RenewControl = codec::decode(raw, ceiling)?;
            let lease = host
                .renew_control(
                    client,
                    view,
                    codec::id(&request.request_id)?,
                    codec::id(&request.lease_id)?,
                    request.generation,
                    now,
                )
                .map_err(|_| TransportError::Authority)?;
            Some((
                p::Kind::ControlGranted,
                granted(lease, request.request_id).encode_to_vec(),
            ))
        }
        p::Kind::ReleaseControl => {
            let request: p::ReleaseControl = codec::decode(raw, ceiling)?;
            host.release_control(
                client,
                codec::id(&request.request_id)?,
                codec::id(&request.lease_id)?,
                request.generation,
                now,
            )
            .map_err(|_| TransportError::Authority)?;
            None
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
            let command = rui_protocol_core::commands::decode_family(family, raw)
                .map_err(|_| TransportError::Protocol)?;
            let result = host
                .submit_family(client, family, raw, now)
                .map_err(|_| TransportError::Protocol)?;
            Some((
                p::Kind::CommandResult,
                codec::command_result(command.header().command_seq, result).encode_to_vec(),
            ))
        }
        p::Kind::GetCommandResult => {
            let request: p::GetCommandResult = codec::decode(raw, ceiling)?;
            let result = host
                .get_command_result(client, view, request.command_seq, now)
                .map_err(|_| TransportError::Authority)?;
            Some((
                p::Kind::CommandResult,
                codec::command_result(request.command_seq, result).encode_to_vec(),
            ))
        }
        p::Kind::CloseView => return Err(TransportError::Closed),
        p::Kind::Ping => {
            let ping: p::Ping = codec::decode(raw, ceiling)?;
            Some((p::Kind::Pong, ping.encode_to_vec()))
        }
        // This controlled producer has no resource closure. Knowing a digest is
        // never retrieval permission; resource-bearing producers must provide
        // their currently authorized closure before this request can be served.
        p::Kind::NeedResources => return Err(TransportError::Authority),
        p::Kind::ResourceAvailable | p::Kind::ResourceEvicted => {
            let _: p::ResourceIds = codec::decode(raw, ceiling)?;
            return Err(TransportError::Authority);
        }
        _ => return Err(TransportError::Protocol),
    };
    drain_host_cleanup(shared, &mut host);
    Ok(result)
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
    fn setup(
        cleanup: Arc<dyn Fn(InputCleanup) + Send + Sync>,
    ) -> (
        Arc<Shared>,
        NegotiatedSession,
        NegotiatedSession,
        [u8; 32],
        [u8; 32],
        VerifiedAuthorization,
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
        let mut host = ControlledHost::new(
            HostConfig {
                max_text_bytes: 64,
                ..Default::default()
            },
            String::new(),
        )
        .unwrap();
        let mut pairing = PairingAuthority::new(2).unwrap();
        let mut sessions = Vec::new();
        let mut grants = Vec::new();
        for id in [3, 4] {
            let request = pairing.request_local_pairing([id; 16]).unwrap();
            let credential = pairing
                .approve_local(request.request_id, auth.clone())
                .unwrap();
            grants.push(
                pairing
                    .approval_id(credential.expose_for_tls_authentication(), [id; 16])
                    .unwrap(),
            );
            let session = host
                .negotiate(
                    auth.clone(),
                    ClientHello {
                        client_instance_id: [id; 16],
                        supported_versions: [1].into_iter().collect(),
                        supported_profiles: SessionConfig::default().supported_profiles,
                        required_profiles: SessionConfig::default().required_profiles,
                        receiver_limits: ReceiverLimits::default(),
                        resume: None,
                    },
                    0,
                )
                .unwrap();
            let payload = host.checkpoint(&session.client, session.view_id).unwrap();
            drop(payload);
            host.acknowledge_applied(&session.client, session.view_id, 1)
                .unwrap();
            host.acknowledge_presented(&session.client, session.view_id, 1)
                .unwrap();
            sessions.push(session);
        }
        let first = sessions.remove(0);
        let second = sessions.remove(0);
        host.acquire_control(&first.client, first.view_id, [9; 16], 0)
            .unwrap();
        let shared = Arc::new(Shared {
            host: Arc::new(Mutex::new(host)),
            pairing: Arc::new(StdMutex::new(pairing)),
            active: StdMutex::new(BTreeSet::new()),
            revoked: StdMutex::new(BTreeMap::new()),
            cleanup_busy: AtomicBool::new(false),
            start: Instant::now(),
            config: ServerConfig {
                cleanup,
                ..Default::default()
            },
        });
        (shared, first, second, grants[0], grants[1], auth)
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn cleanup_completes_before_another_connection_can_acquire() {
        let (entered_tx, entered_rx) = oneshot::channel();
        let entered = StdMutex::new(Some(entered_tx));
        let (release, wait) = std::sync::mpsc::channel();
        let wait = StdMutex::new(wait);
        let callback = Arc::new(move |_: InputCleanup| {
            entered.lock().unwrap().take().unwrap().send(()).unwrap();
            wait.lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(2))
                .unwrap();
        });
        let (shared, _, second, _, grant, auth) = setup(callback);
        shared.host.lock().await.local_activity(0);
        let draining = {
            let shared = shared.clone();
            tokio::spawn(async move {
                drain_cleanup(&shared).await;
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
                    grant,
                    &auth,
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
        let (shared, _, second, _, grant, auth) =
            setup(Arc::new(|_| panic!("trusted cleanup failed")));
        shared.host.lock().await.local_activity(0);
        let task = {
            let shared = shared.clone();
            tokio::spawn(async move {
                drain_cleanup(&shared).await;
            })
        };
        assert!(task.await.unwrap_err().is_panic());
        assert!(shared.cleanup_busy.load(Ordering::Acquire));
        assert_eq!(
            handle(
                &shared,
                &second,
                p::Kind::AcquireControl,
                &p::AcquireControl {
                    request_id: vec![7; 16]
                }
                .encode_to_vec(),
                grant,
                &auth
            )
            .await,
            Err(TransportError::Authority)
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn revocation_while_waiting_for_host_lock_prevents_ledger_admission() {
        let (shared, first, _, grant, _, auth) = setup(Arc::new(|_| {}));
        let locked = shared.host.lock().await;
        let command = rui_protocol_core::wire::proto::SemanticCommand {
            epoch: first.epoch.to_vec(),
            view_id: first.view_id.to_vec(),
            client_instance_id: first.client.client_instance_id.to_vec(),
            command_seq: 1,
            target_id: COUNTER_ID,
            ..Default::default()
        }
        .encode_to_vec();
        let task = {
            let shared = shared.clone();
            tokio::spawn(async move {
                handle(
                    &shared,
                    &first,
                    p::Kind::SemanticCommand,
                    &command,
                    grant,
                    &auth,
                )
                .await
            })
        };
        tokio::task::yield_now().await;
        shared.pairing.lock().unwrap().revoke_local([3; 16]);
        drop(locked);
        assert_eq!(task.await.unwrap(), Err(TransportError::Authentication));
        assert_eq!(shared.host.lock().await.diagnostics().consumed_commands, 0);
    }
    #[tokio::test]
    async fn queued_cleanup_is_drained_by_acquire_before_grant() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = calls.clone();
        let (shared, _, second, _, grant, auth) = setup(Arc::new(move |_| {
            observed.fetch_add(1, Ordering::AcqRel);
        }));
        shared.host.lock().await.local_activity(0);
        assert!(handle(
            &shared,
            &second,
            p::Kind::AcquireControl,
            &p::AcquireControl {
                request_id: vec![7; 16]
            }
            .encode_to_vec(),
            grant,
            &auth
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
}
