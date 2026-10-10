//! Optional transport boundary for one cooperating GTK source owner.
//!
//! Network tasks never borrow the source or ledger. A bounded owner thread
//! performs each compound operation; lifecycle invalidation has an independent
//! atomic path through the inherited native fence.
use super::*;
use rui_protocol_core::session::ResumeDisposition;
use rui_transport::{
    producer::{
        Acknowledgement, AuthorizationLifetime, Connection, Control, LifecycleFuture, Operation,
        PortFuture, ProducerPort, Received, Reply as PortReply, Request, ResourceAccess,
        ResourceData, Revoked, ViewDescription, Witnesses,
    },
    TransportError,
};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, SyncSender},
    Mutex, Weak,
};
use tokio::sync::{oneshot, OwnedSemaphorePermit, Semaphore};

const QUEUE: usize = 32;
const REQUEST_CHARGE: usize = 128 << 10;
const MAX_DIGESTS: usize = 512;

#[derive(Clone)]
pub struct GtkProducerPort(Arc<Inner>);
struct Inner {
    queue: SyncSender<Work>,
    requests: Arc<Semaphore>,
    lifecycle: Arc<Semaphore>,
    priority: Arc<Priority>,
    budget: MemoryBudget,
}
struct Work {
    request: OwnerRequest,
    ingress: u64,
    entered: Instant,
    reply: oneshot::Sender<Result<Received, TransportError>>,
    memory: MemoryReservation,
    _permit: OwnedSemaphorePermit,
    _pending: PendingWork,
}
struct PendingWork(Arc<Priority>);
impl Drop for PendingWork {
    fn drop(&mut self) {
        self.0.pending.fetch_sub(1, Ordering::AcqRel);
        self.0.wake.notify_waiters();
    }
}
enum OwnerRequest {
    Protocol(Request),
    Refresh,
    Restart {
        command: ProcessCommand,
        directory: PathBuf,
    },
    #[cfg(feature = "gtk-fixture-hooks")]
    Fixture(FixtureMutation),
}
struct Priority {
    source_alive: AtomicBool,
    identity: Arc<()>,
    scopes: Mutex<[Option<Arc<Scope>>; MAX_CLIENTS]>,
    fence: Mutex<NativeFence>,
    controller: AtomicU64,
    stopped: AtomicBool,
    now: AtomicU64,
    drained: AtomicU64,
    obligations: AtomicU64,
    pending: AtomicU64,
    #[cfg(test)]
    reply_gate: Mutex<Option<Arc<ReplyGate>>>,
    wake: tokio::sync::Notify,
    _memory: MemoryReservation,
}
struct Scope {
    serial: u64,
    client: AuthenticatedClient,
    view: Id,
    current: AtomicBool,
    opened: AtomicBool,
    lifetime: AuthorizationLifetime,
    resource_lifetime: AuthorizationLifetime,
    revoked: Mutex<Option<Revoked>>,
    priority: Weak<Priority>,
    _memory: ViewMemoryReservation,
}
struct Capability {
    owner: Arc<()>,
    scope: Arc<Scope>,
}
struct OwnerExit(Arc<Priority>);
impl Drop for OwnerExit {
    fn drop(&mut self) {
        self.0.stopped.store(true, Ordering::Release);
        self.0.scan();
        self.0
            .fence
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .invalidate();
        self.0.drained.store(u64::MAX, Ordering::Release);
        self.0.wake.notify_waiters();
    }
}
impl Drop for Capability {
    fn drop(&mut self) {
        self.scope.invalidate();
    }
}
impl Scope {
    fn valid(&self) -> bool {
        self.current.load(Ordering::Acquire) && self.lifetime.is_authorized()
    }
    fn invalidate(&self) {
        let changed = self.current.swap(false, Ordering::AcqRel);
        self.resource_lifetime.revoke();
        if let Some(priority) = self.priority.upgrade() {
            if changed {
                priority.obligations.fetch_add(1, Ordering::AcqRel);
            }
            if priority.controller.load(Ordering::Acquire) == self.serial {
                // Never hold scopes while taking fence. grant_if's authorization
                // closure reads atomics only; no inverse priority lock exists.
                priority
                    .fence
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .invalidate_if(|| priority.controller.load(Ordering::Acquire) == self.serial);
            }
            priority.wake.notify_waiters();
        }
    }
}
impl Priority {
    fn scan(&self) {
        let scopes = self
            .scopes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        for scope in scopes.into_iter().flatten() {
            if self.stopped.load(Ordering::Acquire) || !scope.lifetime.is_authorized() {
                scope.invalidate();
            }
        }
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.priority.stopped.store(true, Ordering::Release);
        self.priority.scan();
    }
}
impl GtkProducerPort {
    /// Consume the host; there is exactly one source/broker/ledger owner.
    pub fn new(host: GtkCommandHost) -> Result<Self, GtkHostError> {
        let budget = host.memory_budget();
        let metadata = budget.reserve(64 << 10)?;
        let owner_stack = budget.reserve(2 << 20)?;
        let watcher_stack = budget.reserve(256 << 10)?;
        let priority = Arc::new(Priority {
            source_alive: AtomicBool::new(host.source_alive),
            identity: Arc::new(()),
            scopes: Mutex::new(std::array::from_fn(|_| None)),
            fence: Mutex::new(host.pipe.fence.clone()),
            controller: AtomicU64::new(0),
            stopped: AtomicBool::new(false),
            now: AtomicU64::new(0),
            drained: AtomicU64::new(0),
            obligations: AtomicU64::new(0),
            pending: AtomicU64::new(0),
            #[cfg(test)]
            reply_gate: Mutex::new(None),
            wake: tokio::sync::Notify::new(),
            _memory: metadata,
        });
        let (sender, receiver) = mpsc::sync_channel(QUEUE);
        let owner_priority = priority.clone();
        std::thread::Builder::new()
            .name("rui-gtk-owner".into())
            .stack_size(1 << 20)
            .spawn(move || {
                let _stack = owner_stack;
                run_owner(host, receiver, owner_priority);
            })?;
        let watcher_priority = priority.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("rui-gtk-lifecycle".into())
            .stack_size(128 << 10)
            .spawn(move || {
                let _stack = watcher_stack;
                while !watcher_priority.stopped.load(Ordering::Acquire) {
                    watcher_priority.scan();
                    std::thread::sleep(Duration::from_millis(5));
                }
                watcher_priority.scan();
            })
        {
            priority.stopped.store(true, Ordering::Release);
            priority.scan();
            return Err(error.into());
        }
        Ok(Self(Arc::new(Inner {
            queue: sender,
            requests: Arc::new(Semaphore::new(QUEUE)),
            lifecycle: Arc::new(Semaphore::new(1)),
            priority,
            budget,
        })))
    }
    pub fn source_alive(&self) -> bool {
        self.0.priority.source_alive.load(Ordering::Acquire)
    }
    pub fn memory_budget(&self) -> MemoryBudget {
        self.0.budget.clone()
    }
    /// Called by the local source integration, never by an unauthenticated peer.
    /// Serialize local publication with protocol commands on the source owner.
    pub fn refresh_if_changed(&self, now: u64) -> PortFuture<'_> {
        self.enqueue(OwnerRequest::Refresh, now, REQUEST_CHARGE)
    }
    /// Trusted source supervisor replacement, serialized with capture/dispatch.
    /// This operation is never exposed as a peer command. Existing views and
    /// exact command ledgers survive; old source authority and identities retire.
    pub fn restart_source(
        &self,
        command: ProcessCommand,
        directory: PathBuf,
        now: u64,
    ) -> PortFuture<'_> {
        let mut bytes = command
            .get_program()
            .as_encoded_bytes()
            .len()
            .saturating_add(directory.as_os_str().as_encoded_bytes().len());
        let mut count = 0usize;
        for arg in command.get_args() {
            count += 1;
            bytes = bytes.saturating_add(arg.as_encoded_bytes().len());
            if count > 128 || bytes > 64 << 10 {
                return Box::pin(async { Err(TransportError::Limit) });
            }
        }
        count = 0;
        for (key, value) in command.get_envs() {
            count += 1;
            bytes = bytes
                .saturating_add(key.as_encoded_bytes().len())
                .saturating_add(value.map_or(0, |v| v.as_encoded_bytes().len()));
            if count > 128 || bytes > 64 << 10 {
                return Box::pin(async { Err(TransportError::Limit) });
            }
        }
        if bytes > 64 << 10 {
            return Box::pin(async { Err(TransportError::Limit) });
        }
        self.enqueue(
            OwnerRequest::Restart { command, directory },
            now,
            REQUEST_CHARGE,
        )
    }
    #[cfg(feature = "gtk-fixture-hooks")]
    pub fn trusted_fixture_mutation(&self, mutation: FixtureMutation, now: u64) -> PortFuture<'_> {
        self.enqueue(OwnerRequest::Fixture(mutation), now, REQUEST_CHARGE)
    }
    fn enqueue(&self, request: OwnerRequest, ingress: u64, bytes: usize) -> PortFuture<'_> {
        let entered = Instant::now();
        if self.0.priority.stopped.load(Ordering::Acquire) {
            return Box::pin(std::future::ready(Err(TransportError::Closed)));
        }
        let permit = match self.0.requests.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => return Box::pin(std::future::ready(Err(TransportError::ProducerBusy))),
        };
        let memory = match self.0.budget.reserve(bytes) {
            Ok(m) => m,
            Err(_) => return Box::pin(std::future::ready(Err(TransportError::Limit))),
        };
        let (reply, receiver) = oneshot::channel();
        self.0.priority.pending.fetch_add(1, Ordering::AcqRel);
        if self
            .0
            .queue
            .try_send(Work {
                request,
                ingress,
                entered,
                reply,
                memory,
                _permit: permit,
                _pending: PendingWork(self.0.priority.clone()),
            })
            .is_err()
        {
            return Box::pin(std::future::ready(Err(TransportError::ProducerBusy)));
        }
        Box::pin(async move { receiver.await.map_err(|_| TransportError::Closed)? })
    }
}
impl ProducerPort for GtkProducerPort {
    fn transact(&self, request: Request) -> PortFuture<'_> {
        let ingress = match &request {
            Request::Connect { now, .. } | Request::Scoped { now, .. } => *now,
        };
        match request_charge(&request) {
            Ok(bytes) => self.enqueue(OwnerRequest::Protocol(request), ingress, bytes),
            Err(error) => Box::pin(std::future::ready(Err(error))),
        }
    }
    fn disconnect(&self, connection: &Connection) {
        if let Some(cap) = connection
            .capability::<Capability>()
            .filter(|cap| Arc::ptr_eq(&cap.owner, &self.0.priority.identity))
        {
            cap.scope.invalidate();
        }
    }
    fn shutdown(&self) {
        self.0.priority.stopped.store(true, Ordering::Release);
        self.0.priority.scan();
    }
    fn flush_lifecycle(&self, now: u64) -> LifecycleFuture<'_> {
        let permit = match self.0.lifecycle.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => return Box::pin(std::future::ready(Err(TransportError::ProducerBusy))),
        };
        let memory = match self.0.budget.reserve(8192) {
            Ok(memory) => memory,
            Err(_) => return Box::pin(std::future::ready(Err(TransportError::Limit))),
        };
        self.0.priority.now.fetch_max(now, Ordering::AcqRel);
        self.0.priority.scan();
        // This is an owner-work barrier as well as a priority cleanup barrier.
        // A canceled Connect may not have issued its cleanup obligation yet:
        // the capability can still be inside an unsent owner reply. Incoming
        // normal traffic can extend this bounded wait; callers stop admission
        // before teardown. Immediate fence invalidation never waits for it.
        Box::pin(async move {
            let _permit = permit;
            let _memory = memory;
            tokio::time::timeout(TIMEOUT + Duration::from_secs(1), async {
                loop {
                    let notified = self.0.priority.wake.notified();
                    tokio::pin!(notified);
                    notified.as_mut().enable();
                    if self.0.priority.pending.load(Ordering::Acquire) == 0 {
                        let target = self.0.priority.obligations.load(Ordering::Acquire);
                        if self.0.priority.drained.load(Ordering::Acquire) >= target {
                            return Ok(());
                        }
                    }
                    notified.await;
                }
            })
            .await
            .map_err(|_| TransportError::Closed)?
        })
    }
}
fn run_owner(mut host: GtkCommandHost, queue: mpsc::Receiver<Work>, priority: Arc<Priority>) {
    let _exit = OwnerExit(priority.clone());
    let mut serial = 0u64;
    let mut issued: [BTreeSet<ResourceDigest>; MAX_CLIENTS] =
        std::array::from_fn(|_| BTreeSet::new());
    let origin = Instant::now();
    let mut clock = 0u64;
    let mut clock_origin = origin;
    let mut clock_started = false;
    loop {
        priority.scan();
        let elapsed = if clock_started {
            u64::try_from(clock_origin.elapsed().as_millis()).unwrap_or(u64::MAX)
        } else {
            0
        };
        let now = clock
            .saturating_add(elapsed)
            .max(priority.now.load(Ordering::Acquire));
        drain(&mut host, &priority, &mut issued, now);
        if priority.stopped.load(Ordering::Acquire) {
            break;
        }
        if host.source_alive {
            let _ = host.refresh_notified();
        }
        priority
            .source_alive
            .store(host.source_alive, Ordering::Release);
        match queue.recv_timeout(Duration::from_millis(5)) {
            Ok(work) => {
                if work.reply.is_closed() {
                    continue;
                }
                let ingress = work.ingress;
                let waited = u64::try_from(work.entered.elapsed().as_millis()).unwrap_or(u64::MAX);
                let Some(request_now) = ingress.checked_add(waited) else {
                    let _ = work.reply.send(Err(TransportError::Limit));
                    continue;
                };
                clock = now.max(request_now);
                clock_origin = Instant::now();
                clock_started = true;
                priority.now.fetch_max(clock, Ordering::AcqRel);
                drain(&mut host, &priority, &mut issued, clock);
                let outcome = match work.request {
                    OwnerRequest::Protocol(request) => execute(
                        &mut host,
                        &priority,
                        &mut serial,
                        &mut issued,
                        request,
                        clock,
                    ),
                    OwnerRequest::Refresh => {
                        if !host.source_alive {
                            Err(TransportError::Closed)
                        } else {
                            host.refresh_if_changed()
                                .map(|_| (PortReply::Done, None))
                                .map_err(host_error)
                        }
                    }
                    OwnerRequest::Restart { command, directory } => {
                        let old = host.native_lease.as_ref().map(|(lease, _)| lease.clone());
                        let result = host.restart_source(command, directory, clock);
                        *priority.fence.lock().unwrap_or_else(|p| p.into_inner()) =
                            host.pipe.fence.clone();
                        priority.controller.store(0, Ordering::Release);
                        if let Some(old) = old {
                            notify_revocation(&priority, &old, "SourceReplacement");
                        }
                        result.map(|_| (PortReply::Done, None)).map_err(host_error)
                    }
                    #[cfg(feature = "gtk-fixture-hooks")]
                    OwnerRequest::Fixture(mutation) => host
                        .trusted_fixture_mutation(mutation)
                        .map(|_| (PortReply::Done, None))
                        .map_err(host_error),
                }
                .map(|(value, view)| Received::charged(value, work.memory, view));
                #[cfg(test)]
                if let Some(gate) = priority.reply_gate.lock().unwrap().take() {
                    gate.pause();
                }
                priority.scan();
                clock = clock.saturating_add(
                    u64::try_from(clock_origin.elapsed().as_millis()).unwrap_or(u64::MAX),
                );
                clock_origin = Instant::now();
                // Failed delivery drops any Connected capability here. Its
                // invalidation must precede the cleanup completion watermark.
                drop(work.reply.send(outcome));
                drain(&mut host, &priority, &mut issued, clock);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    priority.stopped.store(true, Ordering::Release);
    priority.scan();
    drain(&mut host, &priority, &mut issued, clock);
    // Queued canceled operations and source disposal finish before shutdown
    // signals completion; their stack/queue charges survive actual ownership.
    drop(queue);
    drop(host);
    priority.drained.store(u64::MAX, Ordering::Release);
    priority.wake.notify_waiters();
}
fn drain(
    host: &mut GtkCommandHost,
    priority: &Priority,
    issued: &mut [BTreeSet<ResourceDigest>; MAX_CLIENTS],
    now: u64,
) {
    let target = priority.obligations.load(Ordering::Acquire);
    let old_lease = host.native_lease.as_ref().map(|(lease, _)| lease.clone());
    let scopes = priority
        .scopes
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    for (index, scope) in scopes.into_iter().enumerate() {
        if let Some(scope) = scope {
            if !scope.valid() {
                let _ = host.disconnect(&scope.client, now);
                let mut slots = priority.scopes.lock().unwrap_or_else(|p| p.into_inner());
                if slots[index]
                    .as_ref()
                    .is_some_and(|slot| slot.serial == scope.serial)
                {
                    slots[index] = None;
                    issued[index].clear();
                }
            }
        }
    }
    host.sessions.tick(now);
    host.drain_native_cleanup();
    let _ = host.poll_source(now);
    if let Some(old) = old_lease {
        if host
            .native_lease
            .as_ref()
            .is_none_or(|(lease, _)| lease.generation != old.generation)
        {
            notify_revocation(priority, &old, "NativeLifecycle");
        }
    }
    priority.drained.fetch_max(target, Ordering::AcqRel);
    priority.wake.notify_waiters();
}
fn notify_revocation(priority: &Priority, old: &ControlLease, reason: &str) {
    let scopes = priority
        .scopes
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    for scope in scopes
        .into_iter()
        .flatten()
        .filter(|scope| scope.valid() && scope.client == old.client)
    {
        *scope.revoked.lock().unwrap_or_else(|p| p.into_inner()) = Some(Revoked {
            lease: old.lease_id,
            generation: old.generation,
            reason: reason.into(),
        });
    }
}
fn scope_for<'a>(
    priority: &Priority,
    connection: &'a Connection,
) -> Result<&'a Scope, TransportError> {
    let cap = connection
        .capability::<Capability>()
        .filter(|cap| Arc::ptr_eq(&cap.owner, &priority.identity))
        .ok_or(TransportError::Authority)?;
    if !cap.scope.valid() || !connection.authorized() {
        return Err(TransportError::Authentication);
    }
    if cap.scope.client != connection.session().client
        || cap.scope.view != connection.session().view_id
    {
        return Err(TransportError::Authority);
    }
    Ok(&cap.scope)
}
fn execute(
    host: &mut GtkCommandHost,
    priority: &Arc<Priority>,
    serial: &mut u64,
    issued: &mut [BTreeSet<ResourceDigest>; MAX_CLIENTS],
    request: Request,
    now: u64,
) -> Result<(PortReply, Option<ViewMemoryReservation>), TransportError> {
    if priority.stopped.load(Ordering::Acquire) {
        return Err(TransportError::Closed);
    }
    match request {
        Request::Connect {
            authorization,
            hello,
            lifetime,
            fresh_view,
            ..
        } => {
            if !lifetime.is_authorized() {
                return Err(TransportError::Authentication);
            }
            let next_serial = serial.checked_add(1).ok_or(TransportError::Limit)?;
            let index = {
                let slots = priority
                    .scopes
                    .lock()
                    .map_err(|_| TransportError::Authority)?;
                if slots
                    .iter()
                    .flatten()
                    .any(|s| s.client.client_instance_id == hello.client_instance_id)
                {
                    return Err(TransportError::ProducerBusy);
                }
                slots
                    .iter()
                    .position(Option::is_none)
                    .ok_or(TransportError::Limit)?
            };
            if fresh_view {
                let client = AuthenticatedClient {
                    principal: authorization.principal.clone(),
                    client_instance_id: hello.client_instance_id,
                };
                let _ = host.sessions.invalidate_authorization(&client, now);
            }
            let session = host
                .negotiate(authorization, *hello, now)
                .map_err(host_error)?;
            let retained = (|| {
                let memory = host.sessions.reserve_view_memory(
                    &session.client,
                    session.view_id,
                    session_charge(&session),
                )?;
                let scope_memory = host.sessions.reserve_view_memory(
                    &session.client,
                    session.view_id,
                    MAX_DIGESTS * 1024 + 8192,
                )?;
                Ok::<_, SessionError>((memory, scope_memory))
            })();
            let (memory, scope_memory) = match retained {
                Ok(value) => value,
                Err(_) => {
                    let _ = host.disconnect(&session.client, now);
                    return Err(TransportError::Limit);
                }
            };
            let keeper = match &session.resume {
                ResumeDisposition::Accepted {
                    from_revision,
                    through_revision,
                    ..
                } if from_revision == through_revision
                    && *through_revision
                        == host.views[&session.client.client_instance_id].published_revision =>
                {
                    host.views[&session.client.client_instance_id]
                        .installed
                        .clone()
                }
                ResumeDisposition::Accepted { .. } => {
                    let _ = host.disconnect(&session.client, now);
                    return Err(TransportError::Protocol);
                }
                _ => None,
            };
            *serial = next_serial;
            let scope = Arc::new(Scope {
                serial: *serial,
                client: session.client.clone(),
                view: session.view_id,
                current: AtomicBool::new(true),
                opened: AtomicBool::new(false),
                lifetime: lifetime.clone(),
                resource_lifetime: AuthorizationLifetime::new(),
                revoked: Mutex::new(None),
                priority: Arc::downgrade(priority),
                _memory: scope_memory,
            });
            issued[index].clear();
            priority
                .scopes
                .lock()
                .map_err(|_| TransportError::Authority)?[index] = Some(scope.clone());
            let connection = Connection::issued(
                session,
                keeper,
                lifetime,
                Box::new(Capability {
                    owner: priority.identity.clone(),
                    scope,
                }),
                memory,
            );
            if !connection.authorized() {
                return Err(TransportError::Authentication);
            }
            Ok((PortReply::Connected(connection), None))
        }
        Request::Scoped {
            connection,
            operation,
            ..
        } => {
            let scope = scope_for(priority, &connection)?;
            host.view(&scope.client).map_err(host_error)?;
            if !scope.opened.load(Ordering::Acquire)
                && !matches!(
                    &operation,
                    Operation::Open { .. } | Operation::Authorize | Operation::ReserveMemory { .. }
                )
            {
                return Err(TransportError::Protocol);
            }
            let index = priority
                .scopes
                .lock()
                .map_err(|_| TransportError::Authority)?
                .iter()
                .position(|s| s.as_ref().is_some_and(|s| s.serial == scope.serial))
                .ok_or(TransportError::Authority)?;
            let view_memory = if matches!(
                &operation,
                Operation::Open { .. }
                    | Operation::Control(_)
                    | Operation::Command { .. }
                    | Operation::Result { .. }
                    | Operation::Resources { .. }
                    | Operation::Evicted { .. }
            ) {
                Some(
                    host.sessions
                        .reserve_view_memory(&scope.client, scope.view, REQUEST_CHARGE)
                        .map_err(|_| TransportError::Limit)?,
                )
            } else {
                None
            };
            let reply = match operation {
                Operation::Authorize => PortReply::Done,
                Operation::Open {
                    selection,
                    mirror_canvas_policy,
                } => {
                    let policy = if connection.session().profiles.contains("semantics.only.v1") {
                        "semantic-only"
                    } else {
                        "host-canvas"
                    };
                    if selection != "fixture"
                        || mirror_canvas_policy != policy
                        || scope.opened.swap(true, Ordering::AcqRel)
                    {
                        return Err(TransportError::Protocol);
                    }
                    let checkpoint = if matches!(
                        connection.session().resume,
                        ResumeDisposition::Accepted { .. }
                    ) {
                        None
                    } else {
                        host.views
                            .get_mut(&scope.client.client_instance_id)
                            .unwrap()
                            .publication
                            .take()
                    };
                    PortReply::Opened {
                        description: description(host, scope)?,
                        checkpoint,
                    }
                }
                Operation::Ack(ack) => {
                    match ack {
                        Acknowledgement::Applied { revision, witness } => {
                            host.acknowledge_applied(&scope.client, revision, &witness)
                        }
                        Acknowledgement::Presented { revision } => {
                            host.acknowledge_presented(&scope.client, revision)
                        }
                        Acknowledgement::SemanticReady { revision } => {
                            host.sessions
                                .acknowledge_semantic_ready(&scope.client, scope.view, revision)
                                .map_err(|_| TransportError::Authority)?;
                            let view = host
                                .views
                                .get_mut(&scope.client.client_instance_id)
                                .unwrap();
                            view.ready = true;
                            install_replacement(&mut host.sessions, view)
                        }
                    }
                    .map_err(host_error)?;
                    PortReply::Done
                }
                Operation::Control(control) => match control {
                    Control::Acquire { request } => {
                        *priority
                            .fence
                            .lock()
                            .map_err(|_| TransportError::Authority)? = host.pipe.fence.clone();
                        let lease = host
                            .acquire_control_guarded(&scope.client, request, now, || {
                                let valid =
                                    scope.valid() && !priority.stopped.load(Ordering::Acquire);
                                if valid {
                                    priority.controller.store(scope.serial, Ordering::Release);
                                }
                                valid
                            })
                            .map_err(host_error)?;
                        PortReply::Lease(lease)
                    }
                    Control::Release {
                        request,
                        lease,
                        generation,
                    } => {
                        host.release_control(&scope.client, request, lease, generation, now)
                            .map_err(host_error)?;
                        PortReply::Done
                    }
                    Control::Renew {
                        request,
                        lease,
                        generation,
                    } => PortReply::Lease(
                        host.renew_control_guarded(
                            &scope.client,
                            request,
                            lease,
                            generation,
                            now,
                            || scope.valid() && !priority.stopped.load(Ordering::Acquire),
                        )
                        .map_err(host_error)?,
                    ),
                },
                Operation::Command { family, bytes } => PortReply::Command(
                    host.submit(&scope.client, family, &bytes, now)
                        .map_err(host_error)?,
                ),
                Operation::Result { sequence } => PortReply::Command(
                    host.ledger
                        .get_result(&scope.client, host.broker_epoch, sequence, now)
                        .map_err(|_| TransportError::Protocol)?,
                ),
                Operation::Resources { witnesses, digests } => {
                    validate_digests(&digests)?;
                    let new = digests
                        .iter()
                        .filter(|d| !issued[index].contains(*d))
                        .count();
                    if issued[index].len() + new > MAX_DIGESTS {
                        return Err(TransportError::Limit);
                    }
                    let allocation = Arc::new(
                        host.sessions
                            .reserve_view_memory(
                                &scope.client,
                                scope.view,
                                4096 + digests.len() * 1024,
                            )
                            .map_err(|_| TransportError::Limit)?,
                    );
                    let mut resources = Vec::with_capacity(digests.len());
                    for digest in &digests {
                        let resource = select_resource(host, scope, &witnesses, *digest)
                            .ok_or(TransportError::Authority)?;
                        resources.push(ResourceAccess::issued_with_scope(
                            Box::new(resource),
                            connection.lifetime().clone(),
                            scope.resource_lifetime.clone(),
                            allocation.clone(),
                        ));
                    }
                    issued[index].extend(digests);
                    PortReply::Resources(resources)
                }
                Operation::Evicted {
                    witnesses,
                    digests,
                    previously_missing,
                } => {
                    validate_digests(&digests)?;
                    if previously_missing.len() > MAX_DIGESTS {
                        return Err(TransportError::Limit);
                    }
                    let mut missing = Vec::new();
                    let mut retired = Vec::new();
                    for digest in digests {
                        if select_resource(host, scope, &witnesses, digest).is_some() {
                            missing.push(digest);
                        } else if issued[index].contains(&digest) {
                            retired.push(digest);
                        } else {
                            return Err(TransportError::Authority);
                        }
                    }
                    let mut union: BTreeSet<_> = previously_missing.into_iter().collect();
                    union.extend(&missing);
                    if union.len() > MAX_DIGESTS {
                        return Err(TransportError::Limit);
                    }
                    for digest in retired {
                        issued[index].remove(&digest);
                    }
                    if !missing.is_empty() {
                        resources_missing(host, scope)?;
                    }
                    PortReply::Evicted { missing }
                }
                Operation::ResourcesMissing => {
                    resources_missing(host, scope)?;
                    PortReply::Done
                }
                Operation::ReserveMemory { bytes } => PortReply::Memory(
                    host.sessions
                        .reserve_view_memory(&scope.client, scope.view, bytes)
                        .map_err(|_| TransportError::Limit)?,
                ),
                Operation::Pump { take_publication } => {
                    let publication = if take_publication {
                        host.views
                            .get_mut(&scope.client.client_instance_id)
                            .unwrap()
                            .publication
                            .take()
                    } else {
                        None
                    };
                    let revoked = scope
                        .revoked
                        .lock()
                        .map_err(|_| TransportError::Authority)?
                        .take();
                    PortReply::Pump {
                        publication,
                        revoked,
                    }
                }
            };
            if !scope.valid() {
                return Err(TransportError::Authentication);
            }
            Ok((reply, view_memory))
        }
    }
}
fn resources_missing(host: &mut GtkCommandHost, scope: &Scope) -> Result<(), TransportError> {
    host.sessions
        .mark_resources_missing(&scope.client, scope.view)
        .map_err(|_| TransportError::Authority)?;
    host.views
        .get_mut(&scope.client.client_instance_id)
        .unwrap()
        .ready = false;
    Ok(())
}
fn description(host: &GtkCommandHost, scope: &Scope) -> Result<ViewDescription, TransportError> {
    let view = host.view(&scope.client).map_err(host_error)?;
    let semantic = view.negotiated.profiles.contains("semantics.only.v1");
    let canvas = host.last_canvas.ok_or(TransportError::Authority)?;
    let coverage = if !host.source_alive {
        "unavailable"
    } else if semantic {
        "semantic-only"
    } else {
        let state = view
            .consumer
            .state()
            .map_err(|_| TransportError::Authority)?;
        let has_gap = state.records.values().any(|record| {
            matches!(record, Record::Surface(surface) if surface.mapped
                && surface.visual_coverage != rui_protocol_core::model::VisualCoverage::Complete)
        });
        if has_gap {
            "partial"
        } else {
            "complete"
        }
    };
    Ok(ViewDescription {
        width: canvas.width,
        height: canvas.height,
        coverage: coverage.into(),
        input_modes: if view.negotiated.permissions.can_control {
            let mut modes = vec!["semantic.v1".into()];
            if !semantic
                && host.supports_text_composition()
                && view
                    .negotiated
                    .permissions
                    .actions
                    .contains("TextComposition")
            {
                modes.push("text.client.v1".into());
            }
            modes
        } else {
            Vec::new()
        },
        input_devices: Vec::new(),
    })
}
impl ResourceData for PublishedResource {
    fn offer(&self) -> &ResourceOffer {
        &self.offer
    }
    fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
fn select_resource(
    host: &GtkCommandHost,
    scope: &Scope,
    witnesses: &Witnesses,
    digest: ResourceDigest,
) -> Option<PublishedResource> {
    witnesses
        .in_flight
        .as_ref()
        .into_iter()
        .chain(witnesses.applied.as_ref())
        .find_map(|witness| {
            host.publication_resource(&scope.client, witness, digest)
                .ok()
        })
}
fn validate_digests(digests: &[ResourceDigest]) -> Result<(), TransportError> {
    if digests.is_empty() || digests.len() > MAX_DIGESTS {
        return Err(TransportError::Limit);
    }
    let unique: BTreeSet<_> = digests.iter().collect();
    if unique.len() != digests.len() {
        return Err(TransportError::Protocol);
    }
    Ok(())
}
fn host_error(error: GtkHostError) -> TransportError {
    match error {
        GtkHostError::Memory(_) => TransportError::Limit,
        GtkHostError::Io(_) | GtkHostError::NativePipe => TransportError::Io,
        _ => TransportError::Authority,
    }
}
fn session_charge(session: &NegotiatedSession) -> usize {
    let mut bytes = 8192 + session.client.principal.capacity();
    for values in [&session.profiles, &session.permissions.actions] {
        for value in values {
            bytes += 512 + value.capacity();
        }
    }
    if let ResumeDisposition::Accepted { transactions, .. } = &session.resume {
        bytes += transactions.capacity() * std::mem::size_of::<SharedPayload>();
    }
    bytes
}
fn request_charge(request: &Request) -> Result<usize, TransportError> {
    let mut bytes = REQUEST_CHARGE;
    match request {
        Request::Connect {
            authorization,
            hello,
            ..
        } => {
            if authorization.permissions.actions.len() > 512
                || hello.supported_profiles.len() > 512
                || hello.required_profiles.len() > 512
                || authorization.principal.len() > 4096
            {
                return Err(TransportError::Limit);
            }
            bytes = bytes
                .checked_add(authorization.principal.capacity())
                .ok_or(TransportError::Limit)?;
            for value in authorization
                .permissions
                .actions
                .iter()
                .chain(&hello.supported_profiles)
                .chain(&hello.required_profiles)
            {
                bytes = bytes
                    .checked_add(512 + value.capacity())
                    .ok_or(TransportError::Limit)?;
            }
        }
        Request::Scoped { operation, .. } => match operation {
            Operation::Command { bytes: body, .. } => {
                if body.len() > 32 << 10 {
                    return Err(TransportError::Limit);
                }
                bytes = bytes
                    .checked_add(body.capacity())
                    .ok_or(TransportError::Limit)?;
            }
            Operation::Open {
                selection,
                mirror_canvas_policy,
            } => {
                bytes = bytes
                    .checked_add(selection.capacity())
                    .and_then(|n| n.checked_add(mirror_canvas_policy.capacity()))
                    .ok_or(TransportError::Limit)?;
            }
            Operation::Resources { digests, .. } => {
                if digests.len() > MAX_DIGESTS {
                    return Err(TransportError::Limit);
                }
                bytes = bytes
                    .checked_add(digests.capacity() * 32)
                    .ok_or(TransportError::Limit)?;
            }
            Operation::Evicted {
                digests,
                previously_missing,
                ..
            } => {
                if digests.len() > MAX_DIGESTS || previously_missing.len() > MAX_DIGESTS {
                    return Err(TransportError::Limit);
                }
                bytes = bytes
                    .checked_add(digests.capacity() * 32)
                    .and_then(|n| n.checked_add(previously_missing.capacity() * 32))
                    .ok_or(TransportError::Limit)?;
            }
            _ => {}
        },
    }
    Ok(bytes)
}

#[cfg(test)]
#[derive(Default)]
struct ReplyGate {
    entered: AtomicBool,
    released: Mutex<bool>,
    changed: std::sync::Condvar,
}
#[cfg(test)]
impl ReplyGate {
    fn pause(&self) {
        self.entered.store(true, Ordering::Release);
        let released = self.released.lock().unwrap();
        let (released, timeout) = self
            .changed
            .wait_timeout_while(released, Duration::from_secs(2), |released| !*released)
            .unwrap();
        assert!(
            *released && !timeout.timed_out(),
            "bounded owner reply gate expired"
        );
    }
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.changed.notify_all();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use rui_protocol_core::session::Permissions;
    fn fixture() -> GtkProducerPort {
        let mut host = super::super::tests::visual_host();
        // Lifetime-only owner test: native effects are tested on real GTK by
        // the separate isolated fixture. No fake callback/presentation occurs.
        host.pipe.child.wait().unwrap();
        host.pipe.child = ProcessCommand::new("sleep").arg("60").spawn().unwrap();
        GtkProducerPort::new(host).unwrap()
    }
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }
    async fn connect(
        port: &GtkProducerPort,
        id: u8,
        lifetime: AuthorizationLifetime,
    ) -> Connection {
        let reply = port.transact(connect_request(id, lifetime)).await.unwrap();
        let PortReply::Connected(connection) = reply.value() else {
            panic!("connection")
        };
        connection.clone()
    }
    fn connect_request(id: u8, lifetime: AuthorizationLifetime) -> Request {
        let profiles: BTreeSet<_> = [
            "mirror2d.core.v1",
            "semantics.core.v1",
            "content.no-raster.v1",
            "glyph.outline-single.v1",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        Request::Connect {
            authorization: VerifiedAuthorization {
                principal: format!("approved-{id}"),
                policy_version: 1,
                permissions: Permissions {
                    can_view: true,
                    can_control: false,
                    actions: BTreeSet::new(),
                },
            },
            hello: Box::new(ClientHello {
                client_instance_id: [id; 16],
                supported_versions: [1].into_iter().collect(),
                supported_profiles: profiles.clone(),
                required_profiles: profiles,
                receiver_limits: ReceiverLimits::default(),
                resume: None,
            }),
            lifetime,
            fresh_view: true,
            now: 1,
        }
    }
    async fn operation(
        port: &GtkProducerPort,
        connection: &Connection,
        operation: Operation,
    ) -> Result<Received, TransportError> {
        port.transact(Request::Scoped {
            connection: connection.clone(),
            operation,
            now: 1,
        })
        .await
    }
    async fn wait_usage(budget: &MemoryBudget, bytes: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while budget.used() != bytes {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    #[test]
    fn composition_capability_requires_full_visual_controller_and_exact_permission() {
        runtime().block_on(async {
            for (semantic, controller, permitted, expected) in [
                (false, false, true, false),
                (false, true, false, false),
                (true, true, true, false),
                (false, true, true, true),
            ] {
                let port = fixture();
                let mut request = connect_request(9, AuthorizationLifetime::new());
                let Request::Connect {
                    authorization,
                    hello,
                    ..
                } = &mut request
                else {
                    unreachable!()
                };
                authorization.permissions.can_control = controller;
                authorization.permissions.actions = if permitted {
                    ["TextComposition".into()].into_iter().collect()
                } else {
                    BTreeSet::new()
                };
                if semantic {
                    hello.supported_profiles.insert("semantics.only.v1".into());
                    hello.required_profiles = ["semantics.only.v1".into()].into_iter().collect();
                }
                let received = port.transact(request).await.unwrap();
                let PortReply::Connected(connection) = received.value() else {
                    panic!("connection")
                };
                let connection = connection.clone();
                drop(received);
                let opened = operation(
                    &port,
                    &connection,
                    Operation::Open {
                        selection: "fixture".into(),
                        mirror_canvas_policy: if semantic {
                            "semantic-only"
                        } else {
                            "host-canvas"
                        }
                        .into(),
                    },
                )
                .await
                .unwrap();
                let PortReply::Opened { description, .. } = opened.value() else {
                    panic!("description")
                };
                assert_eq!(
                    description
                        .input_modes
                        .iter()
                        .any(|mode| mode == "text.client.v1"),
                    expected
                );
                assert!(description.input_devices.is_empty());
                drop(opened);
                let session = connection.session();
                let bytes = pb::TextComposition {
                    header: Some(pb::CommandHeader {
                        epoch: session.epoch.to_vec(),
                        view_id: session.view_id.to_vec(),
                        client_instance_id: session.client.client_instance_id.to_vec(),
                        command_seq: 1,
                        control_lease_id: [7; 16].to_vec(),
                        control_generation: 1,
                        observed_view_revision: 1,
                    }),
                    target_id: 9,
                    text_session_id: [8; 16].to_vec(),
                    focus_generation: 1,
                    base_text_version: 1,
                    operation: Some(pb::text_composition::Operation::Begin(pb::ScalarRange {
                        start: 0,
                        end: 0,
                    })),
                }
                .encode_to_vec();
                let denied = operation(
                    &port,
                    &connection,
                    Operation::Command {
                        family: CommandFamily::TextComposition,
                        bytes: bytes.into(),
                    },
                )
                .await
                .unwrap();
                let PortReply::Command(result) = denied.value() else {
                    panic!("result")
                };
                assert_eq!(
                    result.status,
                    rui_protocol_core::commands::CommandStatus::Rejected
                );
                assert_eq!(port.0.priority.fence.lock().unwrap().current(), 0);
                drop(denied);
                port.disconnect(&connection);
                port.flush_lifecycle(1).await.unwrap();
                drop(connection);
                port.shutdown();
                port.flush_lifecycle(1).await.unwrap();
            }
        });
    }
    #[test]
    fn gtk_text_clock_profile_is_selected_only_when_client_supports_it() {
        runtime().block_on(async {
            for supported in [false, true] {
                let port = fixture();
                let mut request = connect_request(9, AuthorizationLifetime::new());
                let Request::Connect { hello, .. } = &mut request else {
                    unreachable!()
                };
                if supported {
                    hello
                        .supported_profiles
                        .insert("text.clock.gtk-editable4224.v1".into());
                }
                assert!(!hello
                    .required_profiles
                    .contains("text.clock.gtk-editable4224.v1"));
                let received = port.transact(request).await.unwrap();
                let PortReply::Connected(connection) = received.value() else {
                    panic!("connection")
                };
                assert_eq!(
                    connection
                        .session()
                        .profiles
                        .contains("text.clock.gtk-editable4224.v1"),
                    supported
                );
                let connection = connection.clone();
                drop(received);
                port.disconnect(&connection);
                port.flush_lifecycle(1).await.unwrap();
                drop(connection);
                port.shutdown();
                port.flush_lifecycle(1).await.unwrap();
            }
        });
    }
    #[test]
    fn canceled_connect_and_shutdown_wait_for_unsent_capability_cleanup() {
        runtime().block_on(async {
            for shutdown in [false, true] {
                let port = fixture();
                let budget = port.memory_budget();
                let gate = Arc::new(ReplyGate::default());
                *port.0.priority.reply_gate.lock().unwrap() = Some(gate.clone());
                let connecting = port.transact(connect_request(24, AuthorizationLifetime::new()));
                tokio::time::timeout(Duration::from_secs(2), async {
                    while !gate.entered.load(Ordering::Acquire) {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                })
                .await
                .unwrap();
                drop(connecting);
                if shutdown {
                    port.shutdown();
                }
                let mut flushed = port.flush_lifecycle(1);
                assert!(
                    tokio::time::timeout(Duration::from_millis(20), &mut flushed)
                        .await
                        .is_err()
                );
                gate.release();
                flushed.await.unwrap();
                assert_eq!(port.0.priority.pending.load(Ordering::Acquire), 0);
                if !shutdown {
                    let connection = connect(&port, 24, AuthorizationLifetime::new()).await;
                    assert!(operation(&port, &connection, Operation::Authorize)
                        .await
                        .is_ok());
                    drop(connection);
                    port.shutdown();
                    port.flush_lifecycle(1).await.unwrap();
                }
                drop(port);
                wait_usage(&budget, 0).await;
            }
        });
    }
    #[test]
    fn lower_cold_limits_reject_atomically_without_disturbing_other_views() {
        runtime().block_on(async {
            let port = fixture();
            let budget = port.memory_budget();
            let existing = connect(&port, 21, AuthorizationLifetime::new()).await;
            for limits in [
                ReceiverLimits {
                    snapshot_bytes: 1024,
                    ..Default::default()
                },
                ReceiverLimits {
                    records: 1,
                    ..Default::default()
                },
                ReceiverLimits {
                    ownership_depth: 1,
                    ..Default::default()
                },
                ReceiverLimits {
                    view_memory_bytes: 4096,
                    ..Default::default()
                },
            ] {
                let mut request = connect_request(22, AuthorizationLifetime::new());
                let Request::Connect { hello, .. } = &mut request else {
                    unreachable!()
                };
                hello.receiver_limits = limits;
                assert!(port.transact(request).await.is_err());
                assert!(operation(&port, &existing, Operation::Authorize)
                    .await
                    .is_ok());
                // Failure cannot retain the slot or a partially published view.
                let recovered = connect(&port, 22, AuthorizationLifetime::new()).await;
                port.disconnect(&recovered);
                port.flush_lifecycle(1).await.unwrap();
                drop(recovered);
            }
            drop(existing);
            port.shutdown();
            port.flush_lifecycle(1).await.unwrap();
            drop(port);
            wait_usage(&budget, 0).await;
        });
    }
    #[test]
    fn offline_resume_and_lower_limits_reset_scope_without_weakening_principal_or_ack_binding() {
        runtime().block_on(async {
            let port = fixture();
            let budget = port.memory_budget();
            let old = connect(&port, 23, AuthorizationLifetime::new()).await;
            let opened = operation(
                &port,
                &old,
                Operation::Open {
                    selection: "fixture".into(),
                    mirror_canvas_policy: "host-canvas".into(),
                },
            )
            .await
            .unwrap();
            let PortReply::Opened {
                checkpoint: Some(witness),
                ..
            } = opened.value()
            else {
                panic!("checkpoint")
            };
            let witness = witness.clone();
            // A lost Presented ACK does not become readiness after reconnect.
            operation(
                &port,
                &old,
                Operation::Ack(Acknowledgement::Applied {
                    revision: 1,
                    witness: witness.clone(),
                }),
            )
            .await
            .unwrap();
            let old_view = old.session().view_id;
            port.disconnect(&old);
            port.flush_lifecycle(1).await.unwrap();
            let mut spoof = connect_request(23, AuthorizationLifetime::new());
            let Request::Connect { authorization, .. } = &mut spoof else {
                unreachable!()
            };
            authorization.principal = "spoofed-principal".into();
            assert!(port.transact(spoof).await.is_err());
            let mut request = connect_request(23, AuthorizationLifetime::new());
            let Request::Connect {
                hello, fresh_view, ..
            } = &mut request
            else {
                unreachable!()
            };
            *fresh_view = false;
            hello.resume = Some(rui_protocol_core::session::ResumeRequest {
                epoch: old.session().epoch,
                view_id: old_view,
                acknowledged_revision: 1,
            });
            hello.receiver_limits.envelope_bytes = 128 << 10;
            hello.receiver_limits.chunk_bytes = 4096;
            let reply = port.transact(request).await.unwrap();
            let PortReply::Connected(fresh) = reply.value() else {
                panic!("connected")
            };
            let fresh = fresh.clone();
            assert_ne!(fresh.session().view_id, old_view);
            assert!(matches!(
                fresh.session().resume,
                ResumeDisposition::ResetRequired(
                    rui_protocol_core::session::ResetReason::ViewChanged
                )
            ));
            assert!(fresh.resume_keeper().is_none());
            assert!(operation(&port, &old, Operation::Authorize).await.is_err());
            let opened_fresh = operation(
                &port,
                &fresh,
                Operation::Open {
                    selection: "fixture".into(),
                    mirror_canvas_policy: "host-canvas".into(),
                },
            )
            .await
            .unwrap();
            assert!(matches!(
                opened_fresh.value(),
                PortReply::Opened {
                    checkpoint: Some(_),
                    ..
                }
            ));
            assert!(operation(
                &port,
                &fresh,
                Operation::Ack(Acknowledgement::Applied {
                    revision: 1,
                    witness
                })
            )
            .await
            .is_err());
            assert!(operation(
                &port,
                &fresh,
                Operation::Ack(Acknowledgement::Presented { revision: 1 })
            )
            .await
            .is_err());
            drop(opened_fresh);
            drop(reply);
            drop(opened);
            drop(old);
            drop(fresh);
            port.shutdown();
            port.flush_lifecycle(1).await.unwrap();
            drop(port);
            wait_usage(&budget, 0).await;
        });
    }
    #[test]
    fn escaped_io_reservations_remain_aggregate_charged_across_fresh_scopes_and_owner_drop() {
        runtime().block_on(async {
            let port = fixture();
            let budget = port.memory_budget();
            let mut keepers = Vec::new();
            let mut former_view = None;
            for _ in 0..4 {
                let connection = connect(&port, 11, AuthorizationLifetime::new()).await;
                assert_ne!(former_view, Some(connection.session().view_id));
                former_view = Some(connection.session().view_id);
                let memory = operation(
                    &port,
                    &connection,
                    Operation::ReserveMemory { bytes: 1 << 20 },
                )
                .await
                .unwrap()
                .into_memory()
                .unwrap();
                keepers.push(memory);
                port.disconnect(&connection);
                port.flush_lifecycle(1).await.unwrap();
                drop(connection);
            }
            port.shutdown();
            port.flush_lifecycle(1).await.unwrap();
            drop(port);
            wait_usage(&budget, 4 << 20).await;
            keepers.pop();
            assert_eq!(budget.used(), 3 << 20);
            drop(keepers);
            assert_eq!(budget.used(), 0);
        });
    }
    #[test]
    fn foreign_scope_and_revoked_resource_handles_fail_with_retained_allocations_charged() {
        runtime().block_on(async {
            let first = fixture();
            let second = fixture();
            let first_budget = first.memory_budget();
            let second_budget = second.memory_budget();
            let lifetime = AuthorizationLifetime::new();
            let connection = connect(&first, 12, lifetime.clone()).await;
            assert!(operation(&second, &connection, Operation::Authorize)
                .await
                .is_err());
            let opened = operation(
                &first,
                &connection,
                Operation::Open {
                    selection: "fixture".into(),
                    mirror_canvas_policy: "host-canvas".into(),
                },
            )
            .await
            .unwrap();
            let PortReply::Opened {
                checkpoint: Some(witness),
                description,
            } = opened.value()
            else {
                panic!("checkpoint")
            };
            assert_eq!(description.coverage, "complete");
            let expected_retained = 8192
                + witness
                    .attachment()
                    .unwrap()
                    .get::<PublishedClosure>()
                    .unwrap()
                    .resources
                    .values()
                    .map(|r| r.bytes.len() + 1024)
                    .sum::<usize>()
                + 4096
                + 1024;
            let digest = *witness
                .attachment()
                .unwrap()
                .get::<PublishedClosure>()
                .unwrap()
                .resources
                .keys()
                .next()
                .unwrap();
            let resource_page = operation(
                &first,
                &connection,
                Operation::Resources {
                    witnesses: Witnesses {
                        in_flight: Some(witness.clone()),
                        applied: None,
                    },
                    digests: vec![digest],
                },
            )
            .await
            .unwrap();
            let PortReply::Resources(resources) = resource_page.value() else {
                panic!("resources")
            };
            let retained = resources[0].clone();
            assert!(!retained.bytes().unwrap().is_empty());
            lifetime.revoke();
            assert!(retained.bytes().is_err());
            assert!(retained.offer().is_err());
            assert!(operation(&first, &connection, Operation::Authorize)
                .await
                .is_err());
            drop(resource_page);
            drop(opened);
            drop(connection);
            first.shutdown();
            second.shutdown();
            first.flush_lifecycle(1).await.unwrap();
            second.flush_lifecycle(1).await.unwrap();
            drop(first);
            drop(second);
            wait_usage(&second_budget, 0).await;
            wait_usage(&first_budget, expected_retained).await;
            drop(retained);
            assert_eq!(first_budget.used(), 0);
        });
    }
}
