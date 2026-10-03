//! Compound producer transactions. A producer keeps its source, broker and
//! command ledger together; the transport owns framing and delivery witnesses.
//! Requests have bounded admission and replies retain their allocation charge.
use crate::TransportError;
pub use rui_host_broker::{AuthorizationLifetime, InputDeviceDescription, ViewDescription};
use rui_host_broker::{AuthorizedResource, ControlledHost};
use rui_protocol_core::{
    commands::{AuthenticatedClient, CommandFamily, CommandResult, Id},
    resources::{MemoryBudget, MemoryReservation, ResourceDigest, ResourceOffer},
    session::{
        ClientHello, ControlLease, InputCleanup, NegotiatedSession, ResumeDisposition,
        SharedPayload, VerifiedAuthorization, ViewMemoryReservation,
    },
};
use std::{
    any::Any,
    collections::BTreeSet,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex as StdMutex, Weak,
    },
    time::Instant,
};
use tokio::sync::{Mutex, Notify, Semaphore};
use zeroize::Zeroizing;

const MAX_REQUESTS: usize = 32;
const MAX_RESOURCES: usize = 512;
const REQUEST_MEMORY: usize = 128 * 1024;

pub type PortFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Received, TransportError>> + Send + 'a>>;
pub type LifecycleFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'a>>;

/// An object-safe boundary for a native owner, a cooperating toolkit owner, or
/// the controlled fixture. No callback waits while holding a native lock.
pub trait ProducerPort: Send + Sync {
    fn transact(&self, request: Request) -> PortFuture<'_>;
    /// Priority lifecycle invalidation, available even when normal work is full.
    fn disconnect(&self, connection: &Connection);
    fn shutdown(&self);
    /// Complete priority obligations before a transport disappears. This is a
    /// separate bounded path, independent of normal request admission.
    fn flush_lifecycle(&self, now: u64) -> LifecycleFuture<'_>;
}

/// Immutable issued scope. Its private capability belongs to one producer and
/// one transport incarnation, independently of protocol session/view IDs.
#[derive(Clone)]
pub struct Connection(Arc<ConnectionData>);
struct ConnectionData {
    session: NegotiatedSession,
    resume_keeper: Option<SharedPayload>,
    lifetime: AuthorizationLifetime,
    capability: Box<dyn Any + Send + Sync>,
    _memory: ViewMemoryReservation,
}
impl Connection {
    /// Trusted producer implementation only. The capability must retain its own
    /// lifecycle invalidation and reject stale/foreign owner operations.
    pub fn issued(
        session: NegotiatedSession,
        resume_keeper: Option<SharedPayload>,
        lifetime: AuthorizationLifetime,
        capability: Box<dyn Any + Send + Sync>,
        memory: ViewMemoryReservation,
    ) -> Self {
        Self(Arc::new(ConnectionData {
            session,
            resume_keeper,
            lifetime,
            capability,
            _memory: memory,
        }))
    }
    pub fn session(&self) -> &NegotiatedSession {
        &self.0.session
    }
    pub fn resume_keeper(&self) -> Option<&SharedPayload> {
        self.0.resume_keeper.as_ref()
    }
    pub fn lifetime(&self) -> &AuthorizationLifetime {
        &self.0.lifetime
    }
    pub fn capability<T: Any>(&self) -> Option<&T> {
        self.0.capability.downcast_ref()
    }
    pub fn authorized(&self) -> bool {
        self.0.lifetime.is_authorized()
    }
}
impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProducerConnection")
            .field("view", &self.session().view_id)
            .finish_non_exhaustive()
    }
}

pub enum Request {
    Connect {
        authorization: VerifiedAuthorization,
        hello: Box<ClientHello>,
        lifetime: AuthorizationLifetime,
        fresh_view: bool,
        now: u64,
    },
    Scoped {
        connection: Connection,
        operation: Operation,
        now: u64,
    },
}
pub enum Operation {
    Authorize,
    Open {
        selection: String,
        mirror_canvas_policy: String,
    },
    Ack(Acknowledgement),
    Control(Control),
    Command {
        family: CommandFamily,
        /// Exact private command bytes are zeroed on failure/cancellation too.
        bytes: Zeroizing<Vec<u8>>,
    },
    Result {
        sequence: u64,
    },
    Resources {
        witnesses: Witnesses,
        digests: Vec<ResourceDigest>,
    },
    Evicted {
        witnesses: Witnesses,
        digests: Vec<ResourceDigest>,
        previously_missing: Vec<ResourceDigest>,
    },
    ResourcesMissing,
    ReserveMemory {
        bytes: usize,
    },
    /// Poll the same source/authority and optionally take one publication. The
    /// authorization check and selection occur in one producer transaction.
    Pump {
        take_publication: bool,
    },
}
pub enum Acknowledgement {
    Applied {
        revision: u64,
        witness: SharedPayload,
    },
    Presented {
        revision: u64,
    },
    SemanticReady {
        revision: u64,
    },
}
pub enum Control {
    Acquire {
        request: Id,
    },
    Renew {
        request: Id,
        lease: Id,
        generation: u64,
    },
    Release {
        request: Id,
        lease: Id,
        generation: u64,
    },
}
/// Only delivery's exact in-flight and last-APPLIED publication keepers belong
/// here. Arbitrary retained-history hashes are not resource access authority.
#[derive(Clone, Default)]
pub struct Witnesses {
    pub in_flight: Option<SharedPayload>,
    pub applied: Option<SharedPayload>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revoked {
    pub lease: Id,
    pub generation: u64,
    pub reason: String,
}
pub enum Reply {
    Connected(Connection),
    Opened {
        description: ViewDescription,
        /// Fresh/reset checkpoint, installed atomically with the description.
        checkpoint: Option<SharedPayload>,
    },
    Lease(ControlLease),
    Command(CommandResult),
    Resources(Vec<ResourceAccess>),
    Evicted {
        missing: Vec<ResourceDigest>,
    },
    Pump {
        publication: Option<SharedPayload>,
        revoked: Option<Revoked>,
    },
    Memory(ViewMemoryReservation),
    Done,
}
/// Borrow response values. Immutable publication/resource/connection handles
/// can be cloned while preserving their independent allocation ownership.
pub struct Received {
    value: Reply,
    _memory: MemoryReservation,
    _view_memory: Option<ViewMemoryReservation>,
}
impl Received {
    pub fn charged(
        value: Reply,
        memory: MemoryReservation,
        view_memory: Option<ViewMemoryReservation>,
    ) -> Self {
        Self {
            value,
            _memory: memory,
            _view_memory: view_memory,
        }
    }
    pub fn value(&self) -> &Reply {
        &self.value
    }
    pub fn into_memory(self) -> Result<ViewMemoryReservation, TransportError> {
        match self.value {
            Reply::Memory(memory) => Ok(memory),
            _ => Err(TransportError::Protocol),
        }
    }
}

/// Verified immutable data behind an adapter-owned allocation lifetime.
pub trait ResourceData: Send + Sync {
    fn offer(&self) -> &ResourceOffer;
    fn bytes(&self) -> &[u8];
}
impl ResourceData for AuthorizedResource {
    fn offer(&self) -> &ResourceOffer {
        self.offer()
    }
    fn bytes(&self) -> &[u8] {
        self.bytes()
    }
}
#[derive(Clone)]
pub struct ResourceAccess(Arc<ResourceAccessData>);
struct ResourceAccessData {
    resource: Box<dyn ResourceData>,
    lifetime: AuthorizationLifetime,
    _memory: Arc<ViewMemoryReservation>,
}
impl ResourceAccess {
    pub fn issued(
        resource: Box<dyn ResourceData>,
        lifetime: AuthorizationLifetime,
        memory: Arc<ViewMemoryReservation>,
    ) -> Self {
        Self(Arc::new(ResourceAccessData {
            resource,
            lifetime,
            _memory: memory,
        }))
    }
    pub fn offer(&self) -> Result<&ResourceOffer, TransportError> {
        self.check()?;
        Ok(self.0.resource.offer())
    }
    pub fn bytes(&self) -> Result<&[u8], TransportError> {
        self.check()?;
        Ok(self.0.resource.bytes())
    }
    pub fn check(&self) -> Result<(), TransportError> {
        if self.0.lifetime.is_authorized() {
            Ok(())
        } else {
            Err(TransportError::Authentication)
        }
    }
}

/// Minimal fixture adapter. It keeps the original shared host available for
/// trusted source tests, and uses fixed priority slots for cancelled sockets.
#[derive(Clone)]
pub struct ControlledPort(Arc<ControlledInner>);
struct ControlledInner {
    host: Arc<Mutex<ControlledHost>>,
    cleanup: Arc<dyn Fn(InputCleanup) + Send + Sync>,
    requests: Arc<Semaphore>,
    lifecycle: Arc<Semaphore>,
    memory: MemoryBudget,
    identity: Arc<()>,
    cleanup_failed: Arc<AtomicBool>,
    worker: StdMutex<Option<tokio::task::JoinHandle<()>>>,
    priority: Arc<Priority>,
}
struct Priority {
    state: StdMutex<State>,
    stopped: AtomicBool,
    wake: Notify,
    last_now: std::sync::atomic::AtomicU64,
    _memory: MemoryReservation,
    #[cfg(test)]
    after_flush: StdMutex<Option<TestPause>>,
    #[cfg(test)]
    before_connect: StdMutex<Option<TestPause>>,
    #[cfg(test)]
    before_complete: StdMutex<Option<TestPause>>,
}
#[cfg(test)]
type TestPause = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);
#[cfg(test)]
async fn test_pause(slot: &StdMutex<Option<TestPause>>) {
    let pause = { slot.lock().unwrap().take() };
    if let Some((entered, release)) = pause {
        let _ = entered.send(());
        let _ = release.await;
    }
}
struct State {
    serial: u64,
    slots: Box<[Slot]>,
}
#[derive(Default)]
struct Slot {
    scope: Option<Scope>,
    disconnect: Option<u64>,
    revoked: Option<Revoked>,
}
struct Scope {
    serial: u64,
    client: AuthenticatedClient,
    view: Id,
    current: bool,
    opened: bool,
}
struct Capability {
    owner: Arc<()>,
    slot: usize,
    serial: u64,
    priority: Weak<Priority>,
}
impl Drop for Capability {
    fn drop(&mut self) {
        if let Some(priority) = self.priority.upgrade() {
            priority.disconnect(self.slot, self.serial);
        }
    }
}
impl Priority {
    fn disconnect(&self, index: usize, serial: u64) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(slot) = state.slots.get_mut(index) {
            if let Some(scope) = &mut slot.scope {
                if scope.serial == serial && scope.current {
                    scope.current = false;
                    slot.disconnect = Some(serial);
                    self.wake.notify_one();
                }
            }
        }
    }
}
impl ControlledPort {
    pub fn new(
        host: Arc<Mutex<ControlledHost>>,
        cleanup: Arc<dyn Fn(InputCleanup) + Send + Sync>,
        connections: usize,
    ) -> Result<Self, TransportError> {
        if connections == 0 || connections > 128 {
            return Err(TransportError::Limit);
        }
        let memory = MemoryBudget::new(8 * 1024 * 1024);
        let held = memory
            .reserve(connections * 8192 + 4096)
            .map_err(|_| TransportError::Limit)?;
        let slots = (0..connections)
            .map(|_| Slot::default())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| TransportError::Io)?;
        let priority = Arc::new(Priority {
            state: StdMutex::new(State { serial: 0, slots }),
            stopped: AtomicBool::new(false),
            wake: Notify::new(),
            last_now: std::sync::atomic::AtomicU64::new(0),
            _memory: held,
            #[cfg(test)]
            after_flush: StdMutex::new(None),
            #[cfg(test)]
            before_connect: StdMutex::new(None),
            #[cfg(test)]
            before_complete: StdMutex::new(None),
        });
        let failed = Arc::new(AtomicBool::new(false));
        let worker_host = host.clone();
        let worker_cleanup = cleanup.clone();
        let worker_priority = priority.clone();
        let worker_failed = failed.clone();
        let worker = runtime.spawn(async move {
            loop {
                worker_priority.wake.notified().await;
                let mut host = worker_host.lock().await;
                drain_controlled(
                    &worker_priority,
                    &*worker_cleanup,
                    &worker_failed,
                    &mut host,
                    worker_priority.last_now.load(Ordering::Acquire),
                );
                if worker_priority.stopped.load(Ordering::Acquire)
                    || worker_failed.load(Ordering::Acquire)
                {
                    break;
                }
            }
        });
        Ok(Self(Arc::new(ControlledInner {
            host,
            cleanup,
            requests: Arc::new(Semaphore::new(MAX_REQUESTS)),
            lifecycle: Arc::new(Semaphore::new(1)),
            memory,
            identity: Arc::new(()),
            cleanup_failed: failed,
            worker: StdMutex::new(Some(worker)),
            priority,
        })))
    }
    pub fn host(&self) -> &Arc<Mutex<ControlledHost>> {
        &self.0.host
    }
    fn capability<'a>(&self, connection: &'a Connection) -> Result<&'a Capability, TransportError> {
        let cap = connection
            .capability::<Capability>()
            .filter(|cap| Arc::ptr_eq(&cap.owner, &self.0.identity))
            .ok_or(TransportError::Authority)?;
        if !connection.authorized() {
            return Err(TransportError::Authentication);
        }
        let state = self
            .0
            .priority
            .state
            .lock()
            .map_err(|_| TransportError::Authority)?;
        if !state.slots.get(cap.slot).is_some_and(|slot| {
            slot.scope.as_ref().is_some_and(|scope| {
                scope.serial == cap.serial
                    && scope.current
                    && scope.client == connection.session().client
                    && scope.view == connection.session().view_id
            })
        }) {
            return Err(TransportError::Authority);
        }
        Ok(cap)
    }
    fn drain(&self, host: &mut ControlledHost, now: u64) {
        drain_controlled(
            &self.0.priority,
            &*self.0.cleanup,
            &self.0.cleanup_failed,
            host,
            now,
        );
    }
    async fn execute(
        &self,
        request: Request,
        memory: MemoryReservation,
        enqueued: Instant,
    ) -> Result<Received, TransportError> {
        let ingress = match &request {
            Request::Connect { now, .. } | Request::Scoped { now, .. } => *now,
        };
        let mut host = self.0.host.lock().await;
        // The wire clock is sampled at ingress. Never admit work with that
        // stale time after waiting for the source owner; injected fixture
        // clocks retain their origin while real monotonic waiting is added.
        let now = execution_time(ingress, enqueued)?;
        self.0.priority.last_now.fetch_max(now, Ordering::AcqRel);
        self.drain(&mut host, now);
        if self.0.cleanup_failed.load(Ordering::Acquire) {
            return Err(TransportError::Authority);
        }
        if self.0.priority.stopped.load(Ordering::Acquire) {
            return Err(TransportError::Closed);
        }
        host.tick(now);
        self.drain(&mut host, now);
        if self.0.cleanup_failed.load(Ordering::Acquire) {
            return Err(TransportError::Authority);
        }
        #[cfg(test)]
        if matches!(&request, Request::Connect { .. }) {
            test_pause(&self.0.priority.before_connect).await;
        }
        let result = (|| -> Result<_, TransportError> {
            let (value, view_memory) = match request {
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
                    // Fixed response admission is acquired before mutating the
                    // broker. A cancelled return owns the issued capability Drop.
                    let index = {
                        let state = self
                            .0
                            .priority
                            .state
                            .lock()
                            .map_err(|_| TransportError::Authority)?;
                        if state.slots.iter().any(|slot| {
                            slot.scope.as_ref().is_some_and(|s| {
                                s.current && s.client.client_instance_id == hello.client_instance_id
                            })
                        }) {
                            return Err(TransportError::ProducerBusy);
                        }
                        state
                            .slots
                            .iter()
                            .position(|slot| {
                                slot.disconnect.is_none()
                                    && slot.scope.as_ref().is_none_or(|scope| !scope.current)
                            })
                            .ok_or(TransportError::Limit)?
                    };
                    let mut state = self
                        .0
                        .priority
                        .state
                        .lock()
                        .map_err(|_| TransportError::Authority)?;
                    let serial = state.serial.checked_add(1).ok_or(TransportError::Limit)?;
                    if fresh_view {
                        host.retire_registered_view(
                            &AuthenticatedClient {
                                principal: authorization.principal.clone(),
                                client_instance_id: hello.client_instance_id,
                            },
                            now,
                        )
                        .map_err(|_| TransportError::Authority)?;
                    }
                    let session = host
                        .negotiate_with_lifetime(authorization, *hello, Some(lifetime.clone()), now)
                        .map_err(|_| TransportError::Authority)?;
                    let held = match host.reserve_view_memory(
                        &session.client,
                        session.view_id,
                        session_memory(&session),
                    ) {
                        Ok(held) => held,
                        Err(_) => {
                            let _ = host.disconnect(&session.client, now);
                            return Err(TransportError::Limit);
                        }
                    };
                    let keeper = if matches!(&session.resume, ResumeDisposition::Accepted { from_revision, through_revision, .. } if from_revision == through_revision)
                    {
                        match host.snapshot_publication(&session.client, session.view_id) {
                            Ok(keeper) => Some(keeper),
                            Err(_) => {
                                let _ = host.disconnect(&session.client, now);
                                return Err(TransportError::Limit);
                            }
                        }
                    } else {
                        None
                    };
                    state.serial = serial;
                    state.slots[index] = Slot {
                        scope: Some(Scope {
                            serial,
                            client: session.client.clone(),
                            view: session.view_id,
                            current: true,
                            opened: false,
                        }),
                        ..Default::default()
                    };
                    let connection = Connection::issued(
                        session,
                        keeper,
                        lifetime,
                        Box::new(Capability {
                            owner: self.0.identity.clone(),
                            slot: index,
                            serial,
                            priority: Arc::downgrade(&self.0.priority),
                        }),
                        held,
                    );
                    (Reply::Connected(connection), None)
                }
                Request::Scoped {
                    connection,
                    operation,
                    ..
                } => {
                    let cap = self.capability(&connection)?;
                    let session = connection.session();
                    let client = &session.client;
                    let view = session.view_id;
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
                            host.reserve_view_memory(client, view, REQUEST_MEMORY)
                                .map_err(|_| TransportError::Limit)?,
                        )
                    } else {
                        None
                    };
                    let reply = match operation {
                        Operation::Authorize => {
                            host.view_description(client, view)
                                .map_err(|_| TransportError::Authority)?;
                            Reply::Done
                        }
                        Operation::Open {
                            selection,
                            mirror_canvas_policy,
                        } => {
                            let policy = if session.profiles.contains("semantics.only.v1") {
                                "semantic-only"
                            } else {
                                "host-canvas"
                            };
                            if selection != "fixture" || mirror_canvas_policy != policy {
                                return Err(TransportError::Protocol);
                            }
                            let mut state = self
                                .0
                                .priority
                                .state
                                .lock()
                                .map_err(|_| TransportError::Authority)?;
                            let scope = state.slots[cap.slot]
                                .scope
                                .as_mut()
                                .ok_or(TransportError::Authority)?;
                            if scope.opened {
                                return Err(TransportError::Protocol);
                            }
                            let description = host
                                .view_description(client, view)
                                .map_err(|_| TransportError::Authority)?;
                            let checkpoint =
                                if matches!(session.resume, ResumeDisposition::Accepted { .. }) {
                                    None
                                } else {
                                    Some(
                                        host.checkpoint(client, view)
                                            .map_err(|_| TransportError::Authority)?,
                                    )
                                };
                            scope.opened = true;
                            Reply::Opened {
                                description,
                                checkpoint,
                            }
                        }
                        Operation::Ack(ack) => {
                            match ack {
                                Acknowledgement::Applied { revision, witness } => {
                                    if witness.publication_revision().is_some() {
                                        host.acknowledge_applied_publication(
                                            client, view, revision, &witness,
                                        )
                                    } else {
                                        host.acknowledge_applied(client, view, revision)
                                    }
                                }
                                Acknowledgement::Presented { revision } => {
                                    host.acknowledge_presented(client, view, revision)
                                }
                                Acknowledgement::SemanticReady { revision } => {
                                    host.acknowledge_semantic_ready(client, view, revision)
                                }
                            }
                            .map_err(|_| TransportError::Authority)?;
                            Reply::Done
                        }
                        Operation::Control(control) => match control {
                            Control::Acquire { request } => Reply::Lease(
                                host.acquire_control(client, view, request, now)
                                    .map_err(|_| TransportError::Authority)?,
                            ),
                            Control::Renew {
                                request,
                                lease,
                                generation,
                            } => Reply::Lease(
                                host.renew_control(client, view, request, lease, generation, now)
                                    .map_err(|_| TransportError::Authority)?,
                            ),
                            Control::Release {
                                request,
                                lease,
                                generation,
                            } => {
                                host.release_control(client, request, lease, generation, now)
                                    .map_err(|_| TransportError::Authority)?;
                                Reply::Done
                            }
                        },
                        Operation::Command { family, bytes } => Reply::Command(
                            host.submit_family(client, family, &bytes, now)
                                .map_err(|_| TransportError::Protocol)?,
                        ),
                        Operation::Result { sequence } => Reply::Command(
                            host.get_command_result(client, view, sequence, now)
                                .map_err(|_| TransportError::Authority)?,
                        ),
                        Operation::Resources { witnesses, digests } => {
                            validate_digests(&digests)?;
                            let allocation = Arc::new(
                                host.reserve_view_memory(client, view, 4096 + digests.len() * 1024)
                                    .map_err(|_| TransportError::Limit)?,
                            );
                            let mut resources = Vec::with_capacity(digests.len());
                            for digest in digests {
                                let resource =
                                    select_resource(&host, client, view, &witnesses, digest)
                                        .ok_or(TransportError::Authority)?;
                                resources.push(ResourceAccess::issued(
                                    Box::new(resource),
                                    connection.lifetime().clone(),
                                    allocation.clone(),
                                ));
                            }
                            Reply::Resources(resources)
                        }
                        Operation::Evicted {
                            witnesses,
                            digests,
                            previously_missing,
                        } => {
                            validate_digests(&digests)?;
                            if previously_missing.len() > MAX_RESOURCES {
                                return Err(TransportError::Limit);
                            }
                            let mut missing = Vec::new();
                            let mut retired = Vec::new();
                            for digest in digests {
                                if select_resource(&host, client, view, &witnesses, digest)
                                    .is_some()
                                {
                                    missing.push(digest);
                                } else {
                                    host.authorize_resource_eviction(client, view, digest)
                                        .map_err(|_| TransportError::Authority)?;
                                    retired.push(digest);
                                }
                            }
                            let old: BTreeSet<_> = previously_missing.into_iter().collect();
                            if old.len()
                                + missing
                                    .iter()
                                    .filter(|digest| !old.contains(*digest))
                                    .count()
                                > MAX_RESOURCES
                            {
                                return Err(TransportError::Limit);
                            }
                            // No owner changes before every ID has been validated.
                            host.acknowledge_retired_resource_evictions(client, view, &retired)
                                .map_err(|_| TransportError::Authority)?;
                            if !missing.is_empty() {
                                host.mark_resources_missing(client, view)
                                    .map_err(|_| TransportError::Authority)?;
                            }
                            Reply::Evicted { missing }
                        }
                        Operation::ResourcesMissing => {
                            host.mark_resources_missing(client, view)
                                .map_err(|_| TransportError::Authority)?;
                            Reply::Done
                        }
                        Operation::ReserveMemory { bytes } => Reply::Memory(
                            host.reserve_view_memory(client, view, bytes)
                                .map_err(|_| TransportError::Limit)?,
                        ),
                        Operation::Pump { take_publication } => {
                            host.view_description(client, view)
                                .map_err(|_| TransportError::Authority)?;
                            let publication = if take_publication {
                                host.take_outbound(client, view)
                                    .map_err(|_| TransportError::Authority)?
                            } else {
                                None
                            };
                            let revoked = self
                                .0
                                .priority
                                .state
                                .lock()
                                .map_err(|_| TransportError::Authority)?
                                .slots[cap.slot]
                                .revoked
                                .take();
                            Reply::Pump {
                                publication,
                                revoked,
                            }
                        }
                    };
                    (reply, view_memory)
                }
            };
            Ok((value, view_memory))
        })();
        #[cfg(test)]
        test_pause(&self.0.priority.before_complete).await;
        self.drain(&mut host, now);
        if self.0.cleanup_failed.load(Ordering::Acquire) {
            return Err(TransportError::Authority);
        }
        let (value, view_memory) = result?;
        Ok(Received::charged(value, memory, view_memory))
    }
}
fn drain_controlled(
    priority: &Priority,
    cleanup_callback: &(dyn Fn(InputCleanup) + Send + Sync),
    failed: &AtomicBool,
    host: &mut ControlledHost,
    now: u64,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if failed.load(Ordering::Acquire) {
            return;
        }
        let mut state = priority.state.lock().unwrap_or_else(|p| p.into_inner());
        if priority.stopped.load(Ordering::Acquire) {
            host.local_activity(now);
            for slot in &mut state.slots {
                if let Some(scope) = &mut slot.scope {
                    if scope.current {
                        scope.current = false;
                        slot.disconnect = Some(scope.serial);
                    }
                }
            }
        }
        for slot in &mut state.slots {
            if let Some(serial) = slot.disconnect.take() {
                if let Some(scope) = &slot.scope {
                    if serial == scope.serial {
                        let _ = host.disconnect(&scope.client, now);
                    }
                }
            }
        }
        drop(state);
        while let Some(cleanup) = host.take_cleanup() {
            let notice = Revoked {
                lease: cleanup.lease.lease_id,
                generation: cleanup.lease.generation,
                reason: format!("{:?}", cleanup.reason),
            };
            let client = &cleanup.lease.client;
            let mut state = priority.state.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(slot) = state.slots.iter_mut().find(|slot| {
                slot.scope
                    .as_ref()
                    .is_some_and(|scope| scope.current && scope.client == *client)
            }) {
                slot.revoked = Some(notice);
            }
            drop(state);
            // The trusted callback must complete all families' cleanup before
            // the same locked host can acquire another lease.
            failed.store(true, Ordering::Release);
            cleanup_callback(cleanup);
            failed.store(false, Ordering::Release);
        }
    }));
}
impl Drop for ControlledInner {
    fn drop(&mut self) {
        self.priority.stopped.store(true, Ordering::Release);
        self.priority.wake.notify_one();
    }
}

fn validate_digests(digests: &[ResourceDigest]) -> Result<(), TransportError> {
    if digests.is_empty() || digests.len() > MAX_RESOURCES {
        return Err(TransportError::Limit);
    }
    let mut unique = BTreeSet::new();
    for digest in digests {
        if !unique.insert(digest) {
            return Err(TransportError::Protocol);
        }
    }
    Ok(())
}
fn execution_time(ingress: u64, enqueued: Instant) -> Result<u64, TransportError> {
    ingress
        .checked_add(
            u64::try_from(enqueued.elapsed().as_millis()).map_err(|_| TransportError::Limit)?,
        )
        .ok_or(TransportError::Limit)
}
fn session_memory(session: &NegotiatedSession) -> usize {
    // The two retained string sets have fixed root slack plus conservative
    // node occupancy overhead. History bytes remain independently charged by
    // SharedPayload; charge the returned handle Vec's actual capacity too.
    let mut bytes = 4096 + session.client.principal.capacity();
    for values in [&session.profiles, &session.permissions.actions] {
        if !values.is_empty() {
            bytes += 768;
        }
        for value in values {
            bytes += 512 + value.capacity();
        }
    }
    if let ResumeDisposition::Accepted { transactions, .. } = &session.resume {
        bytes += transactions.capacity() * std::mem::size_of::<SharedPayload>();
    }
    bytes
}
fn select_resource(
    host: &ControlledHost,
    client: &AuthenticatedClient,
    view: Id,
    witnesses: &Witnesses,
    digest: ResourceDigest,
) -> Option<AuthorizedResource> {
    witnesses
        .in_flight
        .as_ref()
        .and_then(|frame| host.publication_resource(client, view, frame, digest).ok())
        .or_else(|| {
            witnesses
                .applied
                .as_ref()
                .and_then(|frame| host.publication_resource(client, view, frame, digest).ok())
        })
}
impl ProducerPort for ControlledPort {
    fn transact(&self, request: Request) -> PortFuture<'_> {
        let enqueued = Instant::now();
        // Acquire bounded admission before allocating the future or retaining
        // any queued request. A socket already charges its exact input body.
        let admitted = match self.0.requests.clone().try_acquire_owned() {
            Ok(admitted) => admitted,
            Err(_) => return Box::pin(std::future::ready(Err(TransportError::Limit))),
        };
        let bytes = match &request {
            Request::Connect {
                authorization,
                hello,
                ..
            } => authorization
                .permissions
                .actions
                .iter()
                .chain(&hello.supported_profiles)
                .chain(&hello.required_profiles)
                .fold(
                    REQUEST_MEMORY.checked_add(authorization.principal.capacity()),
                    |n, s| {
                        n.and_then(|n| n.checked_add(s.capacity()))
                            .and_then(|n| n.checked_add(512))
                    },
                ),
            Request::Scoped {
                operation: Operation::Command { bytes, .. },
                ..
            } => REQUEST_MEMORY.checked_add(bytes.capacity()),
            Request::Scoped {
                operation:
                    Operation::Open {
                        selection,
                        mirror_canvas_policy,
                    },
                ..
            } => REQUEST_MEMORY
                .checked_add(selection.capacity())
                .and_then(|n| n.checked_add(mirror_canvas_policy.capacity())),
            Request::Scoped {
                operation: Operation::Resources { digests, .. },
                ..
            } => digests
                .capacity()
                .checked_mul(32)
                .and_then(|n| n.checked_add(REQUEST_MEMORY)),
            Request::Scoped {
                operation:
                    Operation::Evicted {
                        digests,
                        previously_missing,
                        ..
                    },
                ..
            } => digests
                .capacity()
                .checked_add(previously_missing.capacity())
                .and_then(|n| n.checked_mul(32))
                .and_then(|n| n.checked_add(REQUEST_MEMORY)),
            _ => Some(REQUEST_MEMORY),
        };
        let charged = match bytes
            .ok_or(TransportError::Limit)
            .and_then(|n| self.0.memory.reserve(n).map_err(|_| TransportError::Limit))
        {
            Ok(charged) => charged,
            Err(error) => return Box::pin(std::future::ready(Err(error))),
        };
        Box::pin(async move {
            let _permit = admitted;
            self.execute(request, charged, enqueued).await
        })
    }
    fn disconnect(&self, connection: &Connection) {
        if let Some(cap) = connection
            .capability::<Capability>()
            .filter(|cap| Arc::ptr_eq(&cap.owner, &self.0.identity))
        {
            self.0.priority.disconnect(cap.slot, cap.serial);
        }
    }
    fn shutdown(&self) {
        self.0.priority.stopped.store(true, Ordering::Release);
        self.0.priority.wake.notify_one();
    }
    fn flush_lifecycle(&self, now: u64) -> LifecycleFuture<'_> {
        let enqueued = Instant::now();
        self.0.priority.last_now.fetch_max(now, Ordering::AcqRel);
        self.0.priority.wake.notify_one();
        let admitted = match self.0.lifecycle.clone().try_acquire_owned() {
            Ok(admitted) => admitted,
            Err(_) => return Box::pin(std::future::ready(Err(TransportError::Limit))),
        };
        Box::pin(async move {
            let _admitted = admitted;
            let mut host = self.0.host.lock().await;
            let now = execution_time(now, enqueued)?;
            self.0.priority.last_now.fetch_max(now, Ordering::AcqRel);
            self.drain(&mut host, now);
            drop(host);
            #[cfg(test)]
            test_pause(&self.0.priority.after_flush).await;
            if self.0.priority.stopped.load(Ordering::Acquire) {
                let task = self
                    .0
                    .worker
                    .lock()
                    .map_err(|_| TransportError::Authority)?
                    .take();
                if let Some(task) = task {
                    task.await.map_err(|_| TransportError::Authority)?;
                }
            }
            if self.0.cleanup_failed.load(Ordering::Acquire) {
                Err(TransportError::Authority)
            } else {
                Ok(())
            }
        })
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use rui_host_broker::HostConfig;
    use rui_protocol_core::session::{Permissions, SessionConfig};
    use std::time::Duration;
    fn connect_request(now: u64) -> Request {
        Request::Connect {
            authorization: VerifiedAuthorization {
                principal: "controller".into(),
                policy_version: 1,
                permissions: Permissions {
                    can_view: true,
                    can_control: true,
                    actions: ["Activate".into()].into_iter().collect(),
                },
            },
            hello: Box::new(ClientHello {
                client_instance_id: [3; 16],
                supported_versions: [1].into_iter().collect(),
                supported_profiles: SessionConfig::default().supported_profiles,
                required_profiles: SessionConfig::default().required_profiles,
                receiver_limits: Default::default(),
                resume: None,
            }),
            lifetime: AuthorizationLifetime::new(),
            fresh_view: false,
            now,
        }
    }
    async fn ready(port: &ControlledPort) -> Connection {
        let received = port.transact(connect_request(0)).await.unwrap();
        let Reply::Connected(connection) = received.value() else {
            panic!()
        };
        let connection = connection.clone();
        drop(received);
        let result = port
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
        } = result.value()
        else {
            panic!()
        };
        let frame = frame.clone();
        drop(result);
        for ack in [
            Acknowledgement::Applied {
                revision: 1,
                witness: frame,
            },
            Acknowledgement::Presented { revision: 1 },
        ] {
            port.transact(Request::Scoped {
                connection: connection.clone(),
                operation: Operation::Ack(ack),
                now: 0,
            })
            .await
            .unwrap();
        }
        port.transact(Request::Scoped {
            connection: connection.clone(),
            operation: Operation::Control(Control::Acquire { request: [9; 16] }),
            now: 0,
        })
        .await
        .unwrap();
        connection
    }
    fn setup(cleanup: Arc<dyn Fn(InputCleanup) + Send + Sync>) -> ControlledPort {
        ControlledPort::new(
            Arc::new(Mutex::new(
                ControlledHost::new(HostConfig::default(), String::new()).unwrap(),
            )),
            cleanup,
            2,
        )
        .unwrap()
    }
    #[tokio::test]
    async fn pending_disconnect_cannot_be_overwritten_by_connect_slot_reuse() {
        use std::sync::atomic::AtomicUsize;
        let cleaned = Arc::new(AtomicUsize::new(0));
        let report = cleaned.clone();
        let port = ControlledPort::new(
            Arc::new(Mutex::new(
                ControlledHost::new(HostConfig::default(), String::new()).unwrap(),
            )),
            Arc::new(move |_| {
                report.fetch_add(1, Ordering::AcqRel);
            }),
            1,
        )
        .unwrap();
        let old = ready(&port).await;
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        *port.0.priority.before_connect.lock().unwrap() = Some((entered_tx, release_rx));
        let owner = port.clone();
        let connect = tokio::spawn(async move { owner.transact(connect_request(1)).await });
        entered_rx.await.unwrap(); // initial/tick cleanup finished; host still held
        port.disconnect(&old);
        release_tx.send(()).unwrap();
        assert!(matches!(connect.await.unwrap(), Err(TransportError::Limit)));
        assert_eq!(cleaned.load(Ordering::Acquire), 1);
        assert!(
            !port
                .host()
                .lock()
                .await
                .view_status(&old.session().client, old.session().view_id,)
                .unwrap()
                .connected
        );
        let received = port.transact(connect_request(1)).await.unwrap();
        let Reply::Connected(replacement) = received.value() else {
            panic!()
        };
        assert_eq!(replacement.session().view_id, old.session().view_id);
        port.disconnect(&old);
        port.transact(Request::Scoped {
            connection: replacement.clone(),
            operation: Operation::Authorize,
            now: 1,
        })
        .await
        .unwrap();
        port.shutdown();
        port.flush_lifecycle(1).await.unwrap();
    }
    #[tokio::test]
    async fn expiry_cleanup_panic_poison_precedes_any_new_control_or_source_dispatch() {
        use prost::Message;
        use rui_protocol_core::wire::proto as p;
        let port = setup(Arc::new(|_| panic!("failed native neutralization")));
        let connection = ready(&port).await;
        assert!(matches!(
            port.transact(Request::Scoped {
                connection: connection.clone(),
                operation: Operation::Control(Control::Acquire { request: [10; 16] }),
                now: 5001,
            })
            .await,
            Err(TransportError::Authority)
        ));
        let raw = p::SemanticCommand {
            epoch: connection.session().epoch.to_vec(),
            view_id: connection.session().view_id.to_vec(),
            client_instance_id: connection.session().client.client_instance_id.to_vec(),
            command_seq: 1,
            control_lease_id: vec![9; 16],
            control_generation: 1,
            target_id: rui_host_broker::COUNTER_ID,
            action_id: "Activate".into(),
            action_schema_version: 1,
            observed_view_revision: 1,
            preconditions: Some(p::Preconditions {
                target_version: Some(1),
                ..Default::default()
            }),
            arguments: Some(p::semantic_command::Arguments::NoArguments(
                p::EmptyArguments {},
            )),
        }
        .encode_to_vec();
        assert!(matches!(
            port.transact(Request::Scoped {
                connection: connection.clone(),
                operation: Operation::Command {
                    family: CommandFamily::Semantic,
                    bytes: raw.into(),
                },
                now: 5001,
            })
            .await,
            Err(TransportError::Authority)
        ));
        assert!(matches!(
            port.transact(connect_request(5001)).await,
            Err(TransportError::Authority)
        ));
        let diagnostics = port.host().lock().await.diagnostics();
        assert_eq!(diagnostics.counter, 0);
        assert_eq!(diagnostics.consumed_commands, 0);
        assert!(port.0.cleanup_failed.load(Ordering::Acquire));
        port.shutdown();
        assert_eq!(
            port.flush_lifecycle(5001).await,
            Err(TransportError::Authority)
        );
    }
    #[tokio::test]
    async fn polled_owner_queue_wait_cannot_extend_expired_command_or_acquire_retry() {
        use prost::Message;
        use rui_protocol_core::{commands::CommandStatus, wire::proto as p};
        let port = setup(Arc::new(|_| {}));
        let connection = ready(&port).await;
        let granted = port
            .transact(Request::Scoped {
                connection: connection.clone(),
                operation: Operation::Control(Control::Acquire { request: [9; 16] }),
                now: 0,
            })
            .await
            .unwrap();
        let Reply::Lease(lease) = granted.value() else {
            panic!()
        };
        assert_eq!(lease.expires_at_ms, 5000);
        let raw = p::SemanticCommand {
            epoch: connection.session().epoch.to_vec(),
            view_id: connection.session().view_id.to_vec(),
            client_instance_id: connection.session().client.client_instance_id.to_vec(),
            command_seq: 1,
            control_lease_id: lease.lease_id.to_vec(),
            control_generation: lease.generation,
            target_id: rui_host_broker::COUNTER_ID,
            action_id: "Activate".into(),
            action_schema_version: 1,
            observed_view_revision: 1,
            preconditions: Some(p::Preconditions {
                target_version: Some(1),
                ..Default::default()
            }),
            arguments: Some(p::semantic_command::Arguments::NoArguments(
                p::EmptyArguments {},
            )),
        }
        .encode_to_vec();
        drop(granted);
        let locked = port.host().lock().await;
        let mut command = port.transact(Request::Scoped {
            connection: connection.clone(),
            operation: Operation::Command {
                family: CommandFamily::Semantic,
                bytes: raw.into(),
            },
            now: 4990,
        });
        let mut acquire = port.transact(Request::Scoped {
            connection: connection.clone(),
            operation: Operation::Control(Control::Acquire { request: [9; 16] }),
            now: 4990,
        });
        // Both futures really reached the owner lock before its lease deadline.
        assert!(tokio::time::timeout(Duration::from_millis(1), &mut command)
            .await
            .is_err());
        assert!(tokio::time::timeout(Duration::from_millis(1), &mut acquire)
            .await
            .is_err());
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(locked);
        let received = command.await.unwrap();
        let Reply::Command(result) = received.value() else {
            panic!()
        };
        assert_eq!(result.status, CommandStatus::Rejected);
        assert_eq!(
            result.rejection,
            Some(rui_protocol_core::commands::Rejection::StaleControl)
        );
        assert!(matches!(acquire.await, Err(TransportError::Authority)));
        assert_eq!(port.host().lock().await.diagnostics().counter, 0);
        // The well-formed sequence retains its stable rejected outcome; it
        // never invokes source dispatch or revives the expired grant.
        assert_eq!(port.host().lock().await.diagnostics().consumed_commands, 1);
        port.shutdown();
        port.flush_lifecycle(5020).await.unwrap();
    }
    #[tokio::test]
    async fn final_disconnect_cleanup_panic_cannot_return_success() {
        let port = setup(Arc::new(|_| panic!("failed native neutralization")));
        let connection = ready(&port).await;
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        *port.0.priority.before_complete.lock().unwrap() = Some((entered_tx, release_rx));
        let owner = port.clone();
        let scope = connection.clone();
        let call = tokio::spawn(async move {
            owner
                .transact(Request::Scoped {
                    connection: scope,
                    operation: Operation::Authorize,
                    now: 0,
                })
                .await
        });
        entered_rx.await.unwrap(); // operation succeeded, return not yet published
        port.disconnect(&connection);
        release_tx.send(()).unwrap();
        assert!(matches!(
            call.await.unwrap(),
            Err(TransportError::Authority)
        ));
        assert!(port.0.cleanup_failed.load(Ordering::Acquire));
        port.shutdown();
        assert_eq!(
            port.flush_lifecycle(0).await,
            Err(TransportError::Authority)
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn disconnect_after_busy_flush_drain_has_persistent_cleanup_ownership() {
        let (cleaned_tx, cleaned_rx) = tokio::sync::oneshot::channel();
        let cleaned = StdMutex::new(Some(cleaned_tx));
        let port = setup(Arc::new(move |_| {
            if let Some(sent) = cleaned.lock().unwrap().take() {
                let _ = sent.send(());
            }
        }));
        let connection = ready(&port).await;
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        *port.0.priority.after_flush.lock().unwrap() = Some((entered_tx, release_rx));
        let active = port.clone();
        let first = tokio::spawn(async move { active.flush_lifecycle(0).await });
        entered_rx.await.unwrap(); // first drain finished; its permit is held
        let locked = port.host().lock().await;
        port.disconnect(&connection); // deliberate drain -> handoff race
        assert_eq!(port.flush_lifecycle(0).await, Err(TransportError::Limit));
        drop(locked);
        // No subsequent normal traffic/flush. The precharged worker owns the
        // obligation even though the busy caller returned an explicit error.
        tokio::time::timeout(Duration::from_secs(1), cleaned_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !port
                .host()
                .lock()
                .await
                .view_status(&connection.session().client, connection.session().view_id)
                .unwrap()
                .connected
        );
        release_tx.send(()).unwrap();
        first.await.unwrap().unwrap();
        port.shutdown();
        port.flush_lifecycle(0).await.unwrap();
    }
    #[tokio::test]
    async fn cancelled_flush_and_last_port_drop_do_not_strand_registration() {
        let (cleaned_tx, cleaned_rx) = tokio::sync::oneshot::channel();
        let cleaned = StdMutex::new(Some(cleaned_tx));
        let port = setup(Arc::new(move |_| {
            if let Some(sent) = cleaned.lock().unwrap().take() {
                let _ = sent.send(());
            }
        }));
        let connection = ready(&port).await;
        let host = port.host().clone();
        let budget = port.0.memory.clone();
        let locked = host.lock().await;
        port.disconnect(&connection);
        let mut flush = port.flush_lifecycle(0);
        assert!(tokio::time::timeout(Duration::from_millis(5), &mut flush)
            .await
            .is_err());
        drop(flush);
        drop(port);
        drop(locked);
        tokio::time::timeout(Duration::from_secs(1), cleaned_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !host
                .lock()
                .await
                .view_status(&connection.session().client, connection.session().view_id)
                .unwrap()
                .connected
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            while budget.used() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn acknowledgements_and_authorize_need_no_new_view_heap_under_pressure() {
        let port = setup(Arc::new(|_| {}));
        let connection = ready(&port).await;
        let session = connection.session();
        let used = port
            .host()
            .lock()
            .await
            .retained_memory_bytes(&session.client, session.view_id)
            .unwrap();
        let pressure = port
            .host()
            .lock()
            .await
            .reserve_view_memory(
                &session.client,
                session.view_id,
                session.limits.view_memory_bytes - used,
            )
            .unwrap();
        for operation in [
            Operation::Authorize,
            Operation::Ack(Acknowledgement::Presented { revision: 1 }),
            Operation::Pump {
                take_publication: false,
            },
        ] {
            port.transact(Request::Scoped {
                connection: connection.clone(),
                operation,
                now: 0,
            })
            .await
            .unwrap();
        }
        drop(pressure);
        port.shutdown();
        port.flush_lifecycle(0).await.unwrap();
    }
}
