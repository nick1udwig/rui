//! Bounded native owner. All authority and source objects live on this thread;
//! callers receive a Send port, never a native context or a second ledger.
use crate::{
    backend::Backend,
    topology::{Projection, WindowSpec},
    RegionGrant, ShellError,
};
use prost::Message;
use rui_protocol_core::wire::proto as pb;
use rui_protocol_core::{
    commands::{AuthenticatedClient, CommandLedger, CommandResult, Id, LedgerLimits},
    model::{Affine, Point, Rect, Size},
    resources::{MemoryBudget, MemoryReservation},
    session::{
        ClientHello, ControlLease, NegotiatedSession, ResumeDisposition, SessionBroker,
        SessionConfig, SharedPayload, VerifiedAuthorization,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
        Arc, Mutex, Weak,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
#[cfg(feature = "transport")]
pub mod transport;
const QUEUE: usize = 32;
const LOCAL_QUEUE: usize = 64;
/// Opaque transaction binding issued by the owner after trusted negotiation.
#[derive(Clone)]
pub struct Connection(Arc<ConnectionData>);
struct ConnectionData {
    client: AuthenticatedClient,
    view: Id,
    epoch: Id,
    session: Id,
    priority_slot: usize,
    owner: Arc<()>,
    serial: u64,
    lifecycle: Weak<Priority>,
    _memory: MemoryReservation,
}
impl Drop for ConnectionData {
    fn drop(&mut self) {
        if let Some(priority) = self.lifecycle.upgrade() {
            let _ = priority.disconnect_stamp(
                self.priority_slot,
                ScopeStamp {
                    client: self.client.client_instance_id,
                    view: self.view,
                    serial: self.serial,
                },
            );
        }
    }
}
impl Connection {
    fn stamp(&self) -> ScopeStamp {
        ScopeStamp {
            client: self.client().client_instance_id,
            view: self.view(),
            serial: self.0.serial,
        }
    }
    pub fn client(&self) -> &AuthenticatedClient {
        &self.0.client
    }
    pub fn view(&self) -> Id {
        self.0.view
    }
    pub fn epoch(&self) -> Id {
        self.0.epoch
    }
    pub fn session(&self) -> Id {
        self.0.session
    }
}
/// Replies keep their budget charge until the last owning wrapper is dropped.
pub struct Received {
    value: Response,
    _memory: MemoryReservation,
}
impl Received {
    pub fn value(&self) -> &Response {
        &self.value
    }
}
pub enum Response {
    #[cfg(feature = "transport")]
    Transport(Result<rui_transport::producer::Received, rui_transport::TransportError>),
    #[cfg(feature = "transport")]
    Projected(transport::ProjectedCapture),
    #[cfg(feature = "transport")]
    PresentedRevision(Option<u64>),
    Connected {
        connection: Connection,
        negotiated: NegotiatedSession,
        resume_keeper: Option<SharedPayload>,
    },
    Source(Projection),
    Lease(ControlLease),
    Command(CommandResult),
    Publication(SharedPayload),
    Done,
    Status(OwnerStatus),
    Diagnostics(OwnerDiagnostics),
    SurfaceOpened(u64),
}
#[derive(Clone, Debug)]
pub struct OwnerDiagnostics {
    pub profile: crate::backend::NativeProfile,
    pub native_frames_applied: usize,
    pub native_frames_cancelled: usize,
    pub held_keys: usize,
    pub held_buttons: usize,
    pub held_touches: usize,
    pub local_held_keys: usize,
    pub now_ms: u64,
    pub lease: Option<LeaseDiagnostic>,
    pub last_cleanup: Option<CleanupDiagnostic>,
    pub native_work_generation: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct LeaseDiagnostic {
    pub generation: u64,
    pub deadline_ms: u64,
}
#[derive(Clone, Debug)]
pub struct CleanupDiagnostic {
    pub generation: u64,
    pub reason: rui_protocol_core::session::CleanupReason,
    pub at_ms: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerStatus {
    Ready,
    Cleaning,
    Poisoned,
    Stopped,
}
type ReplyWake = Arc<Mutex<Option<std::task::Waker>>>;
pub struct Pending(Receiver<Result<Received, ShellError>>, ReplyWake);
struct ReplySender(Option<SyncSender<Result<Received, ShellError>>>, ReplyWake);
impl ReplySender {
    fn send(mut self, value: Result<Received, ShellError>) -> Option<Result<Received, ShellError>> {
        self.0
            .take()
            .unwrap()
            .send(value)
            .err()
            .map(|failed| failed.0)
    }
}
impl Drop for ReplySender {
    fn drop(&mut self) {
        drop(self.0.take());
        let waker = self.1.lock().unwrap().take();
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}
impl Pending {
    pub async fn receive(self) -> Result<Received, ShellError> {
        std::future::poll_fn(move |cx| {
            *self.1.lock().unwrap() = Some(cx.waker().clone());
            match self.try_recv() {
                Ok(Some(result)) => std::task::Poll::Ready(result),
                Ok(None) => std::task::Poll::Pending,
                Err(error) => std::task::Poll::Ready(Err(error)),
            }
        })
        .await
    }
    pub fn try_recv(&self) -> Result<Option<Result<Received, ShellError>>, ShellError> {
        match self.0.try_recv() {
            Ok(r) => Ok(Some(r)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(ShellError::Native("owner stopped")),
        }
    }
    /// Blocking helper for a non-GUI worker or explicit test only.
    pub fn wait(self, timeout: Duration) -> Result<Received, ShellError> {
        self.0
            .recv_timeout(timeout)
            .map_err(|_| ShellError::Native("owner reply deadline"))?
    }
}
struct Priority {
    identity: Arc<()>,
    current: Mutex<[Option<ScopeStamp>; 8]>,
    generation: Arc<AtomicU64>,
    quit: AtomicBool,
    local_activity: AtomicBool,
    disconnected: Mutex<[Option<ScopeStamp>; 8]>,
    controller: Mutex<Option<(Id, Id)>>,
    acknowledged_generation: AtomicU64,
    pending_local: AtomicUsize,
    local_failure: AtomicBool,
    owner_failed: AtomicBool,
    local_cancel: AtomicBool,
    local_generation: AtomicU64,
    #[cfg(feature = "transport")]
    stopped: AtomicBool,
    #[cfg(feature = "transport")]
    settled: AtomicBool,
    #[cfg(feature = "transport")]
    pending_requests: AtomicUsize,
    _memory: MemoryReservation,
    #[cfg(test)]
    before_requests: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}
impl Priority {
    fn disconnect_stamp(&self, slot_index: usize, stamp: ScopeStamp) -> Result<(), ShellError> {
        if self.quit.load(Ordering::Acquire) {
            return Err(ShellError::Native("owner stopped"));
        }
        let current = self
            .current
            .lock()
            .map_err(|_| ShellError::Native("scope lock"))?;
        if current[slot_index] != Some(stamp) {
            return Err(ShellError::Native("stale connection"));
        }
        let mut pending = self
            .disconnected
            .lock()
            .map_err(|_| ShellError::Native("priority lock"))?;
        let slot = &mut pending[slot_index];
        if slot.is_none_or(|notice| notice.serial <= stamp.serial) {
            *slot = Some(stamp);
        }
        if self
            .controller
            .lock()
            .map_err(|_| ShellError::Native("priority lock"))?
            .is_some_and(|owner| owner == (stamp.client, stamp.view))
        {
            self.invalidate()?;
        }
        Ok(())
    }
    fn validate(&self, connection: &Connection) -> Result<(), ShellError> {
        if !Arc::ptr_eq(&self.identity, &connection.0.owner)
            || self
                .current
                .lock()
                .map_err(|_| ShellError::Native("scope lock"))?[connection.0.priority_slot]
                != Some(connection.stamp())
        {
            return Err(ShellError::Native("stale or foreign connection"));
        }
        Ok(())
    }
    fn invalidate(&self) -> Result<u64, ShellError> {
        self.generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            .map(|n| n + 1)
            .map_err(|_| {
                self.quit.store(true, Ordering::Release);
                ShellError::Limit
            })
    }
    fn dispatch_ready(&self, generation: u64) -> bool {
        self.pending_local.load(Ordering::Acquire) == 0
            && !self.local_activity.load(Ordering::Acquire)
            && !self.local_failure.load(Ordering::Acquire)
            && !self.local_cancel.load(Ordering::Acquire)
            && generation == self.acknowledged_generation.load(Ordering::Acquire)
            && generation == self.generation.load(Ordering::Acquire)
            && self
                .disconnected
                .lock()
                .is_ok_and(|pending| pending.iter().all(Option::is_none))
    }
}
struct CheckpointKeeper {
    view: Id,
    revision: u64,
    bytes: SharedPayload,
}
struct Request {
    operation: Operation,
    generation: u64,
    reply: ReplySender,
    _memory: MemoryReservation,
    #[cfg(feature = "transport")]
    _obligation: RequestObligation,
}
#[cfg(feature = "transport")]
struct RequestObligation(Arc<Priority>);
#[cfg(feature = "transport")]
impl Drop for RequestObligation {
    fn drop(&mut self) {
        self.0.pending_requests.fetch_sub(1, Ordering::AcqRel);
    }
}
enum Operation {
    #[cfg(feature = "transport")]
    Transport(Box<rui_transport::producer::Request>),
    #[cfg(feature = "transport")]
    Projected {
        connection: Connection,
    },
    #[cfg(feature = "transport")]
    ProjectedSource {
        client: Id,
        view: Id,
    },
    #[cfg(feature = "transport")]
    PresentedSource {
        client: Id,
        view: Id,
    },
    Negotiate {
        auth: VerifiedAuthorization,
        hello: Box<ClientHello>,
    },
    SourceEdit(SourceEdit),
    Capture {
        connection: Option<Connection>,
    },
    Checkpoint {
        connection: Connection,
    },
    Ack {
        connection: Connection,
        revision: u64,
        presented: bool,
    },
    Release {
        connection: Connection,
        lease: Id,
        generation: u64,
    },

    Acquire {
        connection: Connection,
        id: Id,
    },
    Raw {
        connection: Connection,
        bytes: Vec<u8>,
    },
    Status,
    Diagnostics,
}
impl Operation {
    fn connection(&self) -> Option<&Connection> {
        match self {
            Self::Capture { connection } => connection.as_ref(),
            #[cfg(feature = "transport")]
            Self::Projected { connection } => Some(connection),
            Self::Checkpoint { connection }
            | Self::Ack { connection, .. }
            | Self::Release { connection, .. }
            | Self::Acquire { connection, .. }
            | Self::Raw { connection, .. } => Some(connection),
            _ => None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScopeStamp {
    client: Id,
    view: Id,
    serial: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct AuthorityIdentity {
    pub session: Id,
    pub epoch: Id,
}
impl AuthorityIdentity {
    /// A new standalone owner is a fresh protocol epoch. Explicit persistent
    /// authority identities are supplied only by a trusted integrating host.
    fn fresh() -> Result<Self, ShellError> {
        use std::io::Read;
        let mut ids = [0u8; 32];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut ids))
            .map_err(|_| ShellError::Native("owner identity entropy"))?;
        Ok(Self {
            session: ids[..16].try_into().unwrap(),
            epoch: ids[16..].try_into().unwrap(),
        })
    }
}
/// Trusted native source input; this lane cannot be selected by a wire message.
#[derive(Clone, Copy, Debug)]
pub enum LocalEvent {
    Key { code: u32, pressed: bool },
    Pointer { desktop: Point, press: bool },
}
/// Trusted producer policy changes on the same authoritative native topology.
/// This capability is not exposed as a wire command or application-only grant.
pub enum SourceEdit {
    Open(WindowSpec),
    Close(u64),
    Focus(u64),
    Map {
        surface: u64,
        mapped: bool,
    },
    Configure {
        surface: u64,
        desktop_rect: Rect,
        content_size: Size,
        transform: Affine,
        stacking_order: i64,
    },
}
#[derive(Clone)]
pub struct LocalDevicePort {
    events: SyncSender<QueuedLocal>,
    priority: Arc<Priority>,
    host: HostPort,
    grant: RegionGrant,
}
struct QueuedLocal {
    event: LocalEvent,
    generation: u64,
}
impl LocalDevicePort {
    pub fn source_health(&self) -> OwnerStatus {
        if self.priority.quit.load(Ordering::Acquire) {
            OwnerStatus::Stopped
        } else if self.priority.owner_failed.load(Ordering::Acquire) {
            OwnerStatus::Poisoned
        } else if !self
            .priority
            .dispatch_ready(self.priority.generation.load(Ordering::Acquire))
        {
            OwnerStatus::Cleaning
        } else {
            OwnerStatus::Ready
        }
    }
    /// Native focus-loss/unmap/disposal cancels held local physical state via
    /// an independent priority slot, even when the ordinary local FIFO is full.
    pub fn cancel_input(&self) -> Result<(), ShellError> {
        self.priority
            .local_generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            .map_err(|_| {
                self.priority.quit.store(true, Ordering::Release);
                ShellError::Limit
            })?;
        self.priority.local_cancel.store(true, Ordering::Release);
        self.priority.local_activity.store(true, Ordering::Release);
        self.priority.invalidate()?;
        Ok(())
    }
    pub fn source_capture(&self) -> Result<Pending, ShellError> {
        self.host
            .request(2048, || Operation::Capture { connection: None })
    }

    /// Invalidation precedes enqueue even under pressure. A failed local event
    /// is reported, while remote queued work is still immediately cancelled.
    pub fn native_event(&self, event: LocalEvent) -> Result<(), ShellError> {
        let local_generation = self.priority.local_generation.load(Ordering::Acquire);
        if self.priority.quit.load(Ordering::Acquire) {
            return Err(ShellError::Native("owner stopped"));
        }
        if self.priority.owner_failed.load(Ordering::Acquire) {
            return Err(ShellError::Native("native source failed"));
        }
        match event {
            LocalEvent::Key { code, .. } if code > 767 => return Err(ShellError::Limit),
            LocalEvent::Pointer { desktop, .. } => {
                self.grant.map(desktop)?;
            }
            _ => {}
        }
        let reserved = self
            .priority
            .pending_local
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(1).filter(|n| *n <= LOCAL_QUEUE + 1)
            })
            .is_ok();
        self.priority.local_activity.store(true, Ordering::Release);
        if let Err(error) = self.priority.invalidate() {
            if reserved {
                self.priority.pending_local.fetch_sub(1, Ordering::AcqRel);
            }
            return Err(error);
        }
        if !reserved {
            self.priority.local_failure.store(true, Ordering::Release);
            return Err(ShellError::Limit);
        }
        if self
            .events
            .try_send(QueuedLocal {
                event,
                generation: local_generation,
            })
            .is_err()
        {
            self.priority.pending_local.fetch_sub(1, Ordering::AcqRel);
            self.priority.local_failure.store(true, Ordering::Release);
            return Err(ShellError::Limit);
        }
        Ok(())
    }
}
#[derive(Clone)]
pub struct HostPort {
    requests: SyncSender<Request>,
    priority: Arc<Priority>,
    budget: MemoryBudget,
}
impl HostPort {
    fn request(
        &self,
        bytes: usize,
        make: impl FnOnce() -> Operation,
    ) -> Result<Pending, ShellError> {
        if self.priority.quit.load(Ordering::Acquire) {
            return Err(ShellError::Native("owner stopped"));
        }
        let memory = self.budget.reserve(bytes)?;
        #[cfg(feature = "transport")]
        let obligation = {
            self.priority
                .pending_requests
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
                .map_err(|_| ShellError::Limit)?;
            self.priority.settled.store(false, Ordering::Release);
            RequestObligation(self.priority.clone())
        };
        let (reply, response) = mpsc::sync_channel(1);
        let wake = Arc::new(Mutex::new(None));
        let request = Request {
            operation: make(),
            generation: self.priority.generation.load(Ordering::Acquire),
            reply: ReplySender(Some(reply), wake.clone()),
            _memory: memory,
            #[cfg(feature = "transport")]
            _obligation: obligation,
        };
        self.requests
            .try_send(request)
            .map_err(|_| ShellError::Limit)?;
        Ok(Pending(response, wake))
    }
    pub fn negotiate(
        &self,
        auth: &VerifiedAuthorization,
        hello: &ClientHello,
    ) -> Result<Pending, ShellError> {
        if auth.principal.len() > 1024
            || hello.supported_versions.len() > 16
            || auth.permissions.actions.len() > 32
            || auth.permissions.actions.iter().any(|s| s.len() > 128)
            || hello.supported_profiles.len() > 64
            || hello.required_profiles.len() > 64
            || hello
                .supported_profiles
                .iter()
                .chain(&hello.required_profiles)
                .any(|s| s.len() > 256)
        {
            return Err(ShellError::Limit);
        }
        self.request(128 << 10, || Operation::Negotiate {
            auth: auth.clone(),
            hello: Box::new(hello.clone()),
        })
    }
    pub fn source_edit(&self, edit: SourceEdit) -> Result<Pending, ShellError> {
        self.request(4096, || Operation::SourceEdit(edit))
    }
    /// Independent direct-source records, not publication replay. Private
    /// source-window capability uses this too; network authorization is added
    /// by the host-port transaction adapter, never by a remote caller's IDs.
    pub fn source_capture(&self, connection: &Connection) -> Result<Pending, ShellError> {
        self.priority.validate(connection)?;
        self.request(2048, || Operation::Capture {
            connection: Some(connection.clone()),
        })
    }
    pub fn checkpoint(&self, connection: &Connection) -> Result<Pending, ShellError> {
        self.priority.validate(connection)?;
        self.request(2048, || Operation::Checkpoint {
            connection: connection.clone(),
        })
    }
    pub fn applied(&self, connection: &Connection, revision: u64) -> Result<Pending, ShellError> {
        self.priority.validate(connection)?;
        self.request(2048, || Operation::Ack {
            connection: connection.clone(),
            revision,
            presented: false,
        })
    }
    pub fn presented(&self, connection: &Connection, revision: u64) -> Result<Pending, ShellError> {
        self.priority.validate(connection)?;
        self.request(2048, || Operation::Ack {
            connection: connection.clone(),
            revision,
            presented: true,
        })
    }
    pub fn release(
        &self,
        connection: &Connection,
        lease: Id,
        generation: u64,
    ) -> Result<Pending, ShellError> {
        self.priority.validate(connection)?;
        self.request(2048, || Operation::Release {
            connection: connection.clone(),
            lease,
            generation,
        })
    }
    /// Priority lifecycle notice cannot be blocked by the normal queue. The
    /// owner checks the exact view before disconnecting a registered client.
    pub fn disconnect(&self, connection: &Connection) -> Result<(), ShellError> {
        if !Arc::ptr_eq(&self.priority.identity, &connection.0.owner) {
            return Err(ShellError::Native("foreign connection"));
        }
        self.priority
            .disconnect_stamp(connection.0.priority_slot, connection.stamp())
    }
    pub fn acquire_control(&self, connection: &Connection, id: Id) -> Result<Pending, ShellError> {
        self.priority.validate(connection)?;
        self.request(2048, || Operation::Acquire {
            connection: connection.clone(),
            id,
        })
    }
    pub fn submit_raw(&self, connection: &Connection, bytes: &[u8]) -> Result<Pending, ShellError> {
        self.priority.validate(connection)?;
        if bytes.len() > 64 << 10 {
            return Err(ShellError::Limit);
        }
        self.request(bytes.len() + 2048, || Operation::Raw {
            connection: connection.clone(),
            bytes: bytes.to_vec(),
        })
    }
    pub fn status(&self) -> Result<Pending, ShellError> {
        self.request(2048, || Operation::Status)
    }
    /// Bounded trusted diagnostic metadata, excluding source text and credentials.
    pub fn diagnostics(&self) -> Result<Pending, ShellError> {
        self.request(2048, || Operation::Diagnostics)
    }
    pub fn memory_budget(&self) -> MemoryBudget {
        self.budget.clone()
    }
}
pub struct Owner {
    port: HostPort,
    local: LocalDevicePort,
    thread: Option<JoinHandle<()>>,
}
impl Owner {
    pub fn spawn(
        grant: RegionGrant,
        layout: &str,
        windows: Vec<WindowSpec>,
        budget: MemoryBudget,
    ) -> Result<Self, ShellError> {
        Self::spawn_with_identity(grant, layout, windows, budget, AuthorityIdentity::fresh()?)
    }
    pub fn spawn_with_identity(
        grant: RegionGrant,
        layout: &str,
        windows: Vec<WindowSpec>,
        budget: MemoryBudget,
        authority: AuthorityIdentity,
    ) -> Result<Self, ShellError> {
        if windows.is_empty() || windows.len() > 16 || !["us", "de"].contains(&layout) {
            return Err(ShellError::Limit);
        }
        // Trusted opaque authority/ledger allowances are reserved before their
        // bounded maps and retained exact command bodies can be allocated.
        // Broker view/publication/IO allocations retain their own parent
        // reservations; this covers only bounded authority metadata/ledger.
        let actor_memory = budget.reserve((8 + 24) * 1024 * 1024)?;
        let priority_memory = budget.reserve(1 << 20)?;
        let (tx, requests) = mpsc::sync_channel(QUEUE);
        let (local_tx, local_events) = mpsc::sync_channel(LOCAL_QUEUE);
        let priority = Arc::new(Priority {
            identity: Arc::new(()),
            current: Mutex::new([None; 8]),
            generation: Arc::new(AtomicU64::new(1)),
            quit: AtomicBool::new(false),
            local_activity: AtomicBool::new(false),
            disconnected: Mutex::new(std::array::from_fn(|_| None)),
            controller: Mutex::new(None),
            acknowledged_generation: AtomicU64::new(1),
            pending_local: AtomicUsize::new(0),
            local_failure: AtomicBool::new(false),
            owner_failed: AtomicBool::new(false),
            local_cancel: AtomicBool::new(false),
            local_generation: AtomicU64::new(1),
            #[cfg(feature = "transport")]
            stopped: AtomicBool::new(false),
            #[cfg(feature = "transport")]
            settled: AtomicBool::new(true),
            #[cfg(feature = "transport")]
            pending_requests: AtomicUsize::new(0),
            _memory: priority_memory,
            #[cfg(test)]
            before_requests: Mutex::new(None),
        });
        let port = HostPort {
            requests: tx,
            priority: priority.clone(),
            budget: budget.clone(),
        };
        let local = LocalDevicePort {
            events: local_tx,
            priority: priority.clone(),
            host: port.clone(),
            grant,
        };
        let (ready, startup) = mpsc::sync_channel(1);
        let layout = layout.to_owned();
        let owner_budget = budget.clone();
        let owner_priority = priority.clone();
        let thread = thread::Builder::new()
            .name("rui-native-owner".into())
            .spawn(move || {
                // Locals drop in reverse declaration order: retain metadata
                // and ledger allowance through every covered object's Drop.
                let _actor_memory = actor_memory;
                let start = Instant::now();
                let initialized = (|| {
                    let mut backend = Backend::new(grant, &layout, &owner_budget)?;
                    backend.bind_generation(owner_priority.generation.clone())?;
                    let mut main = None;
                    for window in windows {
                        let id = backend.topology_mut().open(window)?;
                        if main.is_none() {
                            main = Some(id);
                        }
                    }
                    backend.topology_mut().focus(main.unwrap())?;
                    let mut config = SessionConfig {
                        max_clients: 8,
                        max_history_bytes: 1 << 20,
                        max_history_records: 64,
                        max_control_requests: 1024,
                        max_held_inputs: 128,
                        ..SessionConfig::default()
                    };
                    config.receiver_limits.snapshot_bytes = 1 << 20;
                    config.receiver_limits.envelope_bytes = 1 << 20;
                    config.receiver_limits.outbound_queue_bytes = 512 << 10;
                    config.receiver_limits.view_memory_bytes = 16 << 20;
                    #[cfg(feature = "transport")]
                    {
                        // View allocations independently retain the shared
                        // parent charge; keep socket work locally bounded too.
                        config.max_clients = 4;
                        // A native mirror retains linear float pixels while
                        // preparing its charged RGBA texture. Thirty-two MiB
                        // admits the normal bounded GTK viewport plus transport
                        // writer headroom; the shared owner budget still caps
                        // all producer views together.
                        config.receiver_limits.view_memory_bytes = 32 << 20;
                    }
                    config.supported_profiles.insert("cursor.vector.v1".into());
                    config.required_profiles.insert("cursor.vector.v1".into());
                    let broker = SessionBroker::with_memory_budget(
                        authority.session,
                        authority.epoch,
                        config,
                        owner_budget.clone(),
                    )?;
                    let limits = LedgerLimits {
                        max_clients: 8,
                        max_commands: 4096,
                        max_retained_command_bytes: 8 << 20,
                        max_command_bytes: 64 << 10,
                        max_outcomes: 512,
                        ..LedgerLimits::default()
                    };
                    let ledger = CommandLedger::new(authority.epoch, limits)?;
                    Ok::<_, ShellError>((backend, broker, ledger))
                })();
                let Ok((mut backend, mut broker, mut ledger)) = initialized else {
                    let _ = ready.send(initialized.map(|_| ()));
                    return;
                };
                let _ = ready.send(Ok(()));
                let mut poisoned = false;
                let mut local_keys = BTreeSet::new();
                let mut last_cleanup = None;
                let mut slots = BTreeMap::<Id, usize>::new();
                let mut identities: [Option<AuthenticatedClient>; 8] =
                    std::array::from_fn(|_| None);
                let mut keepers: [Option<CheckpointKeeper>; 8] = std::array::from_fn(|_| None);
                let mut connection_serial = 0u64;
                #[cfg(feature = "transport")]
                let mut transport = transport::NativeViews::default();
                while !owner_priority.quit.load(Ordering::Acquire) {
                    let processed_generation = owner_priority.generation.load(Ordering::Acquire);
                    let now = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                    let local_activity =
                        owner_priority.local_activity.swap(false, Ordering::AcqRel);
                    #[cfg(feature = "transport")]
                    transport.synchronize(&mut broker, &owner_priority, now);
                    if local_activity && backend.invalidate_local_input(&mut broker, now).is_err() {
                        poisoned = true;
                    }
                    // No native/context lock is held while lifecycle authority
                    // changes; each notice is exact client+view bound.
                    let disconnects = {
                        let mut pending = owner_priority.disconnected.lock().unwrap();
                        std::mem::replace(&mut *pending, std::array::from_fn(|_| None))
                    };
                    for (slot, stamp) in disconnects.into_iter().enumerate() {
                        if let Some(stamp) = stamp {
                            if owner_priority.current.lock().unwrap()[slot] == Some(stamp) {
                                if let Some(client) = &identities[slot] {
                                    let _ = broker.disconnect(client, now);
                                }
                                owner_priority.current.lock().unwrap()[slot] = None;
                            }
                        }
                    }
                    if !poisoned && backend.poll_nonblocking(&mut broker, now).is_err() {
                        poisoned = true;
                    }
                    if !backend.cleanup_pending() && !poisoned {
                        if owner_priority.local_cancel.swap(false, Ordering::AcqRel) {
                            if backend.clear_local_keys().is_err() {
                                poisoned = true;
                            }
                            local_keys.clear();
                        }
                        // The central owner acknowledges common cleanup only after
                        // this native backend is neutral. This source has no IME
                        // composition family; such a record is fail-closed.
                        while let Some(cleanup) = broker.take_cleanup() {
                            last_cleanup = Some(CleanupDiagnostic {
                                generation: cleanup.lease.generation,
                                reason: cleanup.reason.clone(),
                                at_ms: now,
                            });
                            #[cfg(feature = "transport")]
                            transport.revoked(&cleanup);
                            if cleanup.composition.is_some() {
                                poisoned = true;
                                break;
                            }
                        }
                        for _ in 0..LOCAL_QUEUE {
                            let Ok(queued) = local_events.try_recv() else {
                                break;
                            };
                            if queued.generation
                                != owner_priority.local_generation.load(Ordering::Acquire)
                            {
                                owner_priority.pending_local.fetch_sub(1, Ordering::AcqRel);
                                continue;
                            }
                            let event = queued.event;
                            let result = match event {
                                LocalEvent::Key { code, pressed } => {
                                    let repeat = pressed && local_keys.contains(&code);
                                    if pressed && !repeat && local_keys.len() >= 128 {
                                        poisoned = true;
                                        break;
                                    }
                                    if pressed {
                                        local_keys.insert(code);
                                    } else {
                                        local_keys.remove(&code);
                                    }
                                    backend.source_local_key(code, pressed, !repeat)
                                }
                                LocalEvent::Pointer { desktop, press } => {
                                    let result = grant.map(desktop).and_then(|point| {
                                        backend.topology_mut().source_pointer(point)
                                    });
                                    if result.is_ok() && press {
                                        if let Some(surface) = backend.topology().hit_test(desktop)
                                        {
                                            backend.topology_mut().focus(surface).and_then(|()| {
                                                backend.topology_mut().source_button(surface, true)
                                            })
                                        } else {
                                            Ok(())
                                        }
                                    } else {
                                        result
                                    }
                                }
                            };
                            owner_priority.pending_local.fetch_sub(1, Ordering::AcqRel);
                            if result.is_err() {
                                poisoned = true;
                                break;
                            }
                        }
                    }
                    if owner_priority.pending_local.load(Ordering::Acquire) == 0
                        && owner_priority.local_failure.load(Ordering::Acquire)
                    {
                        poisoned = true;
                    }
                    if poisoned {
                        owner_priority.owner_failed.store(true, Ordering::Release);
                    }
                    if !poisoned
                        && !backend.cleanup_pending()
                        && owner_priority.pending_local.load(Ordering::Acquire) == 0
                        && !owner_priority.local_activity.load(Ordering::Acquire)
                    {
                        owner_priority
                            .acknowledged_generation
                            .store(processed_generation, Ordering::Release);
                        #[cfg(feature = "transport")]
                        if owner_priority.pending_requests.load(Ordering::Acquire) == 0 {
                            owner_priority.settled.store(true, Ordering::Release);
                        }
                    }
                    *owner_priority.controller.lock().unwrap() = broker
                        .current_lease(now)
                        .map(|l| (l.client.client_instance_id, l.view_id));
                    #[cfg(test)]
                    if let Some(hook) = owner_priority.before_requests.lock().unwrap().take() {
                        hook();
                    }
                    match requests.recv_timeout(Duration::from_millis(1)) {
                        Ok(request) => {
                            #[cfg(feature = "transport")]
                            owner_priority.settled.store(false, Ordering::Release);
                            let now = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                            let reply_memory =
                                owner_budget.reserve(64 << 10).map_err(ShellError::from);
                            let result = reply_memory.and_then(|memory| {
                                if let Some(connection) = request.operation.connection() {
                                    owner_priority.validate(connection)?;
                                }
                                let value = match request.operation {
                                    #[cfg(feature = "transport")]
                                    Operation::Transport(request_body) => {
                                        Response::Transport(transport.execute(
                                            *request_body,
                                            transport::Context {
                                                backend: &mut backend,
                                                broker: &mut broker,
                                                ledger: &mut ledger,
                                                priority: &owner_priority,
                                                budget: &owner_budget,
                                                slots: &mut slots,
                                                identities: &mut identities,
                                                serial: &mut connection_serial,
                                                poisoned,
                                                local_held: !local_keys.is_empty(),
                                                generation: request.generation,
                                                now,
                                            },
                                        ))
                                    }
                                    #[cfg(feature = "transport")]
                                    Operation::Projected { connection } => Response::Projected(
                                        transport.capture(&connection, &owner_budget, &broker)?,
                                    ),
                                    #[cfg(feature = "transport")]
                                    Operation::ProjectedSource { client, view } => {
                                        Response::Projected(transport.capture_named(
                                            client,
                                            view,
                                            &owner_budget,
                                            &broker,
                                            &owner_priority,
                                        )?)
                                    }
                                    #[cfg(feature = "transport")]
                                    Operation::PresentedSource { client, view } => {
                                        Response::PresentedRevision(transport.presented_named(
                                            client,
                                            view,
                                            &broker,
                                            &owner_priority,
                                        )?)
                                    }
                                    Operation::SourceEdit(edit) => {
                                        let topology = backend.topology_mut();
                                        match edit {
                                            SourceEdit::Open(spec) => {
                                                Response::SurfaceOpened(topology.open(spec)?)
                                            }
                                            SourceEdit::Close(id) => {
                                                topology.close(id)?;
                                                Response::Done
                                            }
                                            SourceEdit::Focus(id) => {
                                                topology.focus(id)?;
                                                Response::Done
                                            }
                                            SourceEdit::Map { surface, mapped } => {
                                                topology.set_mapped(surface, mapped)?;
                                                Response::Done
                                            }
                                            SourceEdit::Configure {
                                                surface,
                                                desktop_rect,
                                                content_size,
                                                transform,
                                                stacking_order,
                                            } => {
                                                topology.configure(
                                                    surface,
                                                    desktop_rect,
                                                    content_size,
                                                    transform,
                                                    stacking_order,
                                                )?;
                                                Response::Done
                                            }
                                        }
                                    }
                                    Operation::Negotiate { mut auth, hello } => {
                                        auth.permissions.actions.retain(|a| a == "RawInputBatch");
                                        let next_slot = slots.len();
                                        if next_slot >= 8
                                            && !slots.contains_key(&hello.client_instance_id)
                                        {
                                            return Err(ShellError::Limit);
                                        }
                                        let held = owner_budget.reserve(8 << 10)?;
                                        let serial = connection_serial
                                            .checked_add(1)
                                            .ok_or(ShellError::Limit)?;
                                        let negotiated =
                                            broker.negotiate(Some(auth), *hello, now)?;
                                        connection_serial = serial;
                                        if owner_priority.controller.lock().unwrap().is_some_and(
                                            |owner| owner.0 == negotiated.client.client_instance_id,
                                        ) {
                                            owner_priority.invalidate()?;
                                        }
                                        let slot = *slots
                                            .entry(negotiated.client.client_instance_id)
                                            .or_insert(next_slot);
                                        let resume_keeper = match &negotiated.resume {
                                            ResumeDisposition::Accepted {
                                                from_revision,
                                                through_revision,
                                                ..
                                            } if from_revision == through_revision => {
                                                let keeper =
                                                    keepers[slot].as_ref().filter(|keeper| {
                                                        keeper.view == negotiated.view_id
                                                            && keeper.revision == *through_revision
                                                    });
                                                if let Some(keeper) = keeper {
                                                    Some(keeper.bytes.clone())
                                                } else {
                                                    let _ =
                                                        broker.disconnect(&negotiated.client, now);
                                                    owner_priority.current.lock().unwrap()[slot] =
                                                        None;
                                                    return Err(ShellError::Native(
                                                        "missing exact resume keeper",
                                                    ));
                                                }
                                            }
                                            ResumeDisposition::Accepted { .. } => {
                                                let _ = broker.disconnect(&negotiated.client, now);
                                                owner_priority.current.lock().unwrap()[slot] = None;
                                                return Err(ShellError::Native(
                                                    "actor has no delta history",
                                                ));
                                            }
                                            _ => None,
                                        };
                                        if keepers[slot]
                                            .as_ref()
                                            .is_some_and(|keeper| keeper.view != negotiated.view_id)
                                        {
                                            keepers[slot] = None;
                                        }
                                        let connection = Connection(Arc::new(ConnectionData {
                                            client: negotiated.client.clone(),
                                            view: negotiated.view_id,
                                            epoch: negotiated.epoch,
                                            session: negotiated.session_id,
                                            priority_slot: slot,
                                            owner: owner_priority.identity.clone(),
                                            serial,
                                            lifecycle: Arc::downgrade(&owner_priority),
                                            _memory: held,
                                        }));
                                        owner_priority.current.lock().unwrap()[slot] =
                                            Some(connection.stamp());
                                        identities[slot] = Some(connection.client().clone());
                                        Response::Connected {
                                            connection,
                                            negotiated,
                                            resume_keeper,
                                        }
                                    }
                                    Operation::Capture { connection } => {
                                        if let Some(connection) = connection {
                                            broker.authorize_projection(
                                                connection.client(),
                                                connection.view(),
                                            )?;
                                        }
                                        Response::Source(backend.topology().snapshot()?)
                                    }
                                    Operation::Checkpoint { connection } => {
                                        broker.authorize_projection(
                                            connection.client(),
                                            connection.view(),
                                        )?;
                                        let source = backend.topology().snapshot()?;
                                        let _staging = owner_budget.reserve(1 << 20)?;
                                        let revision = source.state().revision;
                                        let payload = pb::Snapshot {
                                            revision,
                                            records: source.records().to_vec(),
                                            required_features: vec!["cursor.vector.v1".into()],
                                            ..Default::default()
                                        }
                                        .encode_to_vec();
                                        let bytes = pb::Envelope {
                                            protocol_version: 1,
                                            session_id: connection.session().to_vec(),
                                            epoch: connection.epoch().to_vec(),
                                            view_id: connection.view().to_vec(),
                                            kind: 1,
                                            payload,
                                            required_features: vec!["cursor.vector.v1".into()],
                                        }
                                        .encode_to_vec();
                                        if bytes.len() > 1 << 20 {
                                            return Err(ShellError::Limit);
                                        }
                                        // The body retains both view and aggregate
                                        // charge before the checkpoint changes. A
                                        // concurrent escaped parent allocation can
                                        // only make install fail atomically.
                                        let payload = broker.snapshot_publication_payload(
                                            connection.client(),
                                            connection.view(),
                                            bytes,
                                            None,
                                        )?;
                                        broker.install_snapshot(
                                            connection.client(),
                                            connection.view(),
                                            revision,
                                            source.state().estimated_bytes(),
                                        )?;
                                        keepers[connection.0.priority_slot] =
                                            Some(CheckpointKeeper {
                                                view: connection.view(),
                                                revision,
                                                bytes: payload.clone(),
                                            });
                                        Response::Publication(payload)
                                    }
                                    Operation::Ack {
                                        connection,
                                        revision,
                                        presented,
                                    } => {
                                        if presented {
                                            broker.acknowledge_presented(
                                                connection.client(),
                                                connection.view(),
                                                revision,
                                            )?;
                                        } else {
                                            broker.acknowledge_applied(
                                                connection.client(),
                                                connection.view(),
                                                revision,
                                                true,
                                            )?;
                                        }
                                        Response::Done
                                    }
                                    Operation::Release {
                                        connection,
                                        lease,
                                        generation,
                                    } => {
                                        broker.release_control(
                                            connection.client(),
                                            connection.view(),
                                            lease,
                                            generation,
                                            now,
                                        )?;
                                        Response::Done
                                    }

                                    Operation::Status => Response::Status(if poisoned {
                                        OwnerStatus::Poisoned
                                    } else if backend.cleanup_pending()
                                        || !owner_priority.dispatch_ready(
                                            owner_priority.generation.load(Ordering::Acquire),
                                        )
                                    {
                                        OwnerStatus::Cleaning
                                    } else {
                                        OwnerStatus::Ready
                                    }),
                                    Operation::Diagnostics => {
                                        use crate::backend::Receipt;
                                        Response::Diagnostics(OwnerDiagnostics {
                                            profile: backend.profile().clone(),
                                            native_frames_applied: backend
                                                .traces()
                                                .filter(|t| t.receipt == Receipt::AppliedToSource)
                                                .count(),
                                            native_frames_cancelled: backend
                                                .traces()
                                                .filter(|t| t.receipt == Receipt::Cancelled)
                                                .count(),
                                            held_keys: backend.held().keys.len(),
                                            held_buttons: backend.held().buttons.len(),
                                            held_touches: backend.held().touches.len(),
                                            local_held_keys: local_keys.len(),
                                            now_ms: now,
                                            lease: broker.current_lease(now).map(|lease| {
                                                LeaseDiagnostic {
                                                    generation: lease.generation,
                                                    deadline_ms: lease.expires_at_ms,
                                                }
                                            }),
                                            last_cleanup: last_cleanup.clone(),
                                            native_work_generation: owner_priority
                                                .generation
                                                .load(Ordering::Acquire),
                                        })
                                    }
                                    Operation::Acquire { connection, id } => {
                                        if poisoned
                                            || backend.cleanup_pending()
                                            || !local_keys.is_empty()
                                            || !owner_priority.dispatch_ready(request.generation)
                                        {
                                            return Err(ShellError::Native(
                                                "native cleanup pending",
                                            ));
                                        }
                                        Response::Lease(broker.acquire_control(
                                            connection.client(),
                                            connection.view(),
                                            id,
                                            now,
                                        )?)
                                    }
                                    Operation::Raw { connection, bytes } => {
                                        if poisoned
                                            || backend.cleanup_pending()
                                            || !owner_priority.dispatch_ready(request.generation)
                                        {
                                            return Err(ShellError::Native(
                                                "stale or unready native work",
                                            ));
                                        }
                                        Response::Command(backend.submit_raw_at_generation(
                                            &mut broker,
                                            &mut ledger,
                                            connection.client(),
                                            &bytes,
                                            now,
                                            request.generation,
                                        )?)
                                    }
                                };
                                Ok(Received {
                                    value,
                                    _memory: memory,
                                })
                            });
                            if let Some(Ok(received)) = request.reply.send(result) {
                                if let Response::Connected { connection, .. } = received.value() {
                                    if owner_priority.validate(connection).is_ok() {
                                        let _ = broker.disconnect(connection.client(), now);
                                        owner_priority.current.lock().unwrap()
                                            [connection.0.priority_slot] = None;
                                    }
                                }
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
                let now = start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                let _ = backend.invalidate_local_input(&mut broker, now);
                let deadline = Instant::now() + Duration::from_secs(2);
                while backend.cleanup_pending() && Instant::now() < deadline {
                    if backend.poll_nonblocking(&mut broker, now).is_err() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                owner_priority.quit.store(true, Ordering::Release);
                #[cfg(feature = "transport")]
                owner_priority.stopped.store(true, Ordering::Release);
                // Remaining requests cannot revive the terminal owner; response
                // channel disconnection informs waiters. Native objects drop here.
            })
            .map_err(|_| ShellError::Native("owner thread creation"))?;
        let startup = startup
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| ShellError::Native("owner startup deadline"))
            .and_then(|result| result);
        if let Err(error) = startup {
            priority.quit.store(true, Ordering::Release);
            let _ = priority.invalidate();
            let _ = thread.join();
            return Err(error);
        }
        Ok(Self {
            port,
            local,
            thread: Some(thread),
        })
    }
    pub fn port(&self) -> HostPort {
        self.port.clone()
    }
    pub fn local_device(&self) -> LocalDevicePort {
        self.local.clone()
    }
    pub fn shutdown(mut self) {
        self.port.priority.quit.store(true, Ordering::Release);
        let _ = self.port.priority.invalidate();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.port.priority.quit.store(true, Ordering::Release);
        let _ = self.port.priority.invalidate();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rui_protocol_core::{
        model::{Affine, Color, Rect, Size, SurfaceRole},
        session::{Permissions, ReceiverLimits},
    };
    fn wait(pending: Pending) -> Received {
        pending.wait(Duration::from_secs(2)).unwrap()
    }
    fn setup() -> (
        Owner,
        HostPort,
        LocalDevicePort,
        Connection,
        ControlLease,
        u64,
        u64,
        Vec<u8>,
    ) {
        let grant = RegionGrant::new(
            crate::InjectionScope::IsolatedWholeSession,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 200.0,
                height: 120.0,
            },
            Rect {
                x: 40.0,
                y: 80.0,
                width: 600.0,
                height: 240.0,
            },
            crate::Framebuffer {
                width: 1000,
                height: 700,
            },
            2.0,
        )
        .unwrap();
        let owner = Owner::spawn_with_identity(
            grant,
            "de",
            vec![WindowSpec {
                role: SurfaceRole::Toplevel,
                parent: None,
                modal_for: None,
                desktop_rect: Rect {
                    x: 10.0,
                    y: 10.0,
                    width: 120.0,
                    height: 90.0,
                },
                content_size: Size {
                    width: 120.0,
                    height: 90.0,
                },
                transform: Affine {
                    xx: 1.0,
                    yy: 1.0,
                    xy: 0.0,
                    yx: 0.0,
                    tx: 0.0,
                    ty: 0.0,
                },
                stacking_order: 0,
                color: Color {
                    red: 0.0,
                    green: 0.0,
                    blue: 1.0,
                    alpha: 1.0,
                },
            }],
            MemoryBudget::new(128 << 20),
            AuthorityIdentity {
                session: [71; 16],
                epoch: [72; 16],
            },
        )
        .unwrap();
        let port = owner.port();
        let local = owner.local_device();
        let config = SessionConfig::default();
        let mut profiles = config.supported_profiles;
        profiles.insert("cursor.vector.v1".into());
        let reply = wait(
            port.negotiate(
                &VerifiedAuthorization {
                    principal: "race-test".into(),
                    policy_version: 1,
                    permissions: Permissions {
                        can_view: true,
                        can_control: true,
                        actions: ["RawInputBatch".into()].into_iter().collect(),
                    },
                },
                &ClientHello {
                    client_instance_id: [9; 16],
                    supported_versions: [1].into_iter().collect(),
                    supported_profiles: profiles,
                    required_profiles: config.required_profiles,
                    receiver_limits: ReceiverLimits::default(),
                    resume: None,
                },
            )
            .unwrap(),
        );
        let Response::Connected { connection, .. } = reply.value() else {
            panic!()
        };
        let c = connection.clone();
        let reply = wait(port.checkpoint(&c).unwrap());
        let Response::Publication(bytes) = reply.value() else {
            panic!()
        };
        let envelope = pb::Envelope::decode(bytes.as_ref()).unwrap();
        let snapshot = pb::Snapshot::decode(envelope.payload.as_slice()).unwrap();
        wait(port.applied(&c, snapshot.revision).unwrap());
        wait(port.presented(&c, snapshot.revision).unwrap());
        let reply = wait(port.acquire_control(&c, [8; 16]).unwrap());
        let Response::Lease(lease) = reply.value() else {
            panic!()
        };
        let lease = lease.clone();
        let reply = wait(port.source_capture(&c).unwrap());
        let Response::Source(source) = reply.value() else {
            panic!()
        };
        let surface = source
            .state()
            .records
            .values()
            .find_map(|record| match record {
                rui_protocol_core::model::Record::Surface(s) => Some(s),
                _ => None,
            })
            .unwrap();
        let id = surface.id;
        let token = surface.routing_token.clone();
        let bytes = raw(&c, &lease, snapshot.revision, id, token.clone(), 1, 42);
        let reply = wait(port.submit_raw(&c, &bytes).unwrap());
        let Response::Command(result) = reply.value() else {
            panic!()
        };
        assert_eq!(
            result.status,
            rui_protocol_core::commands::CommandStatus::Dispatched
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let reply = wait(port.diagnostics().unwrap());
            let Response::Diagnostics(d) = reply.value() else {
                panic!()
            };
            if d.held_keys == 1 {
                break;
            }
            assert!(Instant::now() < deadline);
        }
        (owner, port, local, c, lease, snapshot.revision, id, token)
    }
    fn raw(
        c: &Connection,
        lease: &ControlLease,
        revision: u64,
        surface: u64,
        token: Vec<u8>,
        seq: u64,
        code: u32,
    ) -> Vec<u8> {
        pb::RawInputBatch {
            header: Some(pb::CommandHeader {
                epoch: c.epoch().to_vec(),
                view_id: c.view().to_vec(),
                client_instance_id: c.client().client_instance_id.to_vec(),
                command_seq: seq,
                control_lease_id: lease.lease_id.to_vec(),
                control_generation: lease.generation,
                observed_view_revision: revision,
            }),
            surface_id: surface,
            routing_token: token,
            presented_revision: revision,
            seat_id: crate::backend::SEAT.into(),
            device_id: crate::backend::DEVICE.into(),
            key_namespace: "linux.evdev".into(),
            events: vec![pb::RawEvent {
                timestamp_us: seq * 1000,
                event: Some(pb::raw_event::Event::Key(pb::KeyEvent {
                    code,
                    pressed: true,
                })),
            }],
        }
        .encode_to_vec()
    }
    fn pause(port: &HostPort) -> (Receiver<()>, SyncSender<()>) {
        let (entered, entered_rx) = mpsc::sync_channel(1);
        let (resume, resume_rx) = mpsc::sync_channel(1);
        *port.priority.before_requests.lock().unwrap() = Some(Box::new(move || {
            entered.send(()).unwrap();
            let _ = resume_rx.recv_timeout(Duration::from_secs(2));
        }));
        (entered_rx, resume)
    }
    #[test]
    #[ignore = "explicit pinned native owner interleaving gate"]
    fn newly_stamped_raw_and_acquire_cannot_overtake_unprocessed_local_obligation() {
        for acquire in [false, true] {
            let (owner, port, local, c, lease, revision, surface, token) = setup();
            let (entered, resume) = pause(&port);
            entered.recv_timeout(Duration::from_secs(2)).unwrap();
            local
                .native_event(LocalEvent::Key {
                    code: 21,
                    pressed: true,
                })
                .unwrap();
            let pending = if acquire {
                port.acquire_control(&c, [7; 16]).unwrap()
            } else {
                port.submit_raw(&c, &raw(&c, &lease, revision, surface, token, 2, 21))
                    .unwrap()
            };
            // Both carry the new generation while the old lease and Shift are
            // still active. Neither may admit before the local obligation.
            assert_eq!(port.priority.generation.load(Ordering::Acquire), 2);
            assert_eq!(
                port.priority
                    .acknowledged_generation
                    .load(Ordering::Acquire),
                1
            );
            resume.send(()).unwrap();
            assert!(pending.wait(Duration::from_secs(2)).is_err());
            local
                .native_event(LocalEvent::Key {
                    code: 21,
                    pressed: false,
                })
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let reply = wait(port.source_capture(&c).unwrap());
                let Response::Source(source) = reply.value() else {
                    panic!()
                };
                let value = source
                    .state()
                    .records
                    .values()
                    .find_map(|r| match r {
                        rui_protocol_core::model::Record::Semantic(s) => match &s.value {
                            Some(rui_protocol_core::model::TypedValue::Text(t)) => Some(t.as_str()),
                            _ => None,
                        },
                        _ => None,
                    })
                    .unwrap();
                if value == "z" {
                    break;
                }
                assert!(Instant::now() < deadline);
            }
            let reply = wait(port.diagnostics().unwrap());
            let Response::Diagnostics(d) = reply.value() else {
                panic!()
            };
            assert_eq!(d.held_keys, 0);
            owner.shutdown();
        }
    }

    #[test]
    #[ignore = "explicit pinned native owner interleaving gate"]
    fn full_local_fifo_finishes_accepted_events_then_reports_terminal_poison() {
        let (owner, port, local, c, _, _, _, _) = setup();
        let (entered, resume) = pause(&port);
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        for index in 0..LOCAL_QUEUE {
            local
                .native_event(LocalEvent::Pointer {
                    desktop: Point {
                        x: 20.0 + index as f64,
                        y: 40.0,
                    },
                    press: false,
                })
                .unwrap();
        }
        assert!(local
            .native_event(LocalEvent::Pointer {
                desktop: Point { x: 90.0, y: 40.0 },
                press: false
            })
            .is_err());
        assert_eq!(
            port.priority.pending_local.load(Ordering::Acquire),
            LOCAL_QUEUE
        );
        resume.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let reply = wait(port.status().unwrap());
            if matches!(reply.value(), Response::Status(OwnerStatus::Poisoned)) {
                break;
            }
            assert!(Instant::now() < deadline);
        }
        assert_eq!(port.priority.pending_local.load(Ordering::Acquire), 0);
        let reply = wait(port.source_capture(&c).unwrap());
        let Response::Source(source) = reply.value() else {
            panic!()
        };
        let position = source
            .state()
            .records
            .values()
            .find_map(|r| match r {
                rui_protocol_core::model::Record::Session(s) => {
                    s.cursor.as_ref().map(|c| c.position)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(position, Point { x: 83.0, y: 40.0 });
        let reply = wait(port.diagnostics().unwrap());
        let Response::Diagnostics(d) = reply.value() else {
            panic!()
        };
        assert_eq!(d.held_keys, 0);
        assert!(port
            .acquire_control(&c, [6; 16])
            .unwrap()
            .wait(Duration::from_secs(2))
            .is_err());
        owner.shutdown();
    }

    fn connect_request(port: &HostPort, id: Id, principal: &str) -> Pending {
        connect_with_resume(port, id, principal, None)
    }
    fn connect_with_resume(
        port: &HostPort,
        id: Id,
        principal: &str,
        resume: Option<rui_protocol_core::session::ResumeRequest>,
    ) -> Pending {
        let config = SessionConfig::default();
        let mut profiles = config.supported_profiles;
        profiles.insert("cursor.vector.v1".into());
        port.negotiate(
            &VerifiedAuthorization {
                principal: principal.into(),
                policy_version: 1,
                permissions: Permissions {
                    can_view: true,
                    can_control: true,
                    actions: ["RawInputBatch".into()].into_iter().collect(),
                },
            },
            &ClientHello {
                client_instance_id: id,
                supported_versions: [1].into_iter().collect(),
                supported_profiles: profiles,
                required_profiles: config.required_profiles,
                receiver_limits: ReceiverLimits::default(),
                resume,
            },
        )
        .unwrap()
    }
    #[test]
    #[ignore = "explicit pinned native owner connection-capability gate"]
    fn replacement_same_view_rejects_old_operations_and_late_disconnect_cannot_overwrite_new() {
        let (owner, port, _, old, _, _, _, _) = setup();
        let reply = wait(connect_request(
            &port,
            old.client().client_instance_id,
            &old.client().principal,
        ));
        let Response::Connected { connection, .. } = reply.value() else {
            panic!()
        };
        let new = connection.clone();
        assert_eq!(old.view(), new.view());
        assert_ne!(old.0.serial, new.0.serial);
        assert!(port.source_capture(&old).is_err());
        assert!(port.applied(&old, 1).is_err());
        assert!(port.disconnect(&old).is_err());
        wait(port.source_capture(&new).unwrap());
        let (entered, resume) = pause(&port);
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        port.disconnect(&new).unwrap();
        assert!(port.disconnect(&old).is_err());
        assert_eq!(
            port.priority.disconnected.lock().unwrap()[new.0.priority_slot],
            Some(new.stamp())
        );
        resume.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while port.priority.current.lock().unwrap()[new.0.priority_slot].is_some() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(port.source_capture(&new).is_err());
        owner.shutdown();
    }
    #[test]
    #[ignore = "explicit pinned native owner connection-capability gate"]
    fn connection_from_other_owner_is_rejected_even_with_identical_fixture_protocol_ids() {
        let (owner, port, _, first, _, _, _, _) = setup();
        let (other_owner, other_port, _, second, _, _, _, _) = setup();
        assert_eq!(first.session(), second.session());
        assert_eq!(first.epoch(), second.epoch());
        assert_eq!(first.view(), second.view());
        assert!(port.source_capture(&second).is_err());
        assert!(port.disconnect(&second).is_err());
        assert!(other_port.checkpoint(&first).is_err());
        wait(port.source_capture(&first).unwrap());
        wait(other_port.source_capture(&second).unwrap());
        owner.shutdown();
        other_owner.shutdown();
    }
    #[test]
    #[ignore = "explicit pinned native owner connection-capability gate"]
    fn cancelled_connect_before_and_after_reply_releases_exact_registration() {
        let (owner, port, _, _, _, _, _, _) = setup();
        for after_reply in [false, true] {
            let (entered, resume) = pause(&port);
            entered.recv_timeout(Duration::from_secs(2)).unwrap();
            let pending = connect_request(&port, [10; 16], "cancelled-client");
            if !after_reply {
                drop(pending);
                resume.send(()).unwrap();
            } else {
                resume.send(()).unwrap();
                let deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    if port
                        .priority
                        .current
                        .lock()
                        .unwrap()
                        .iter()
                        .flatten()
                        .any(|s| s.client == [10; 16])
                    {
                        break;
                    }
                    assert!(Instant::now() < deadline);
                    std::thread::yield_now();
                }
                drop(pending);
            }
            wait(port.status().unwrap());
            assert!(!port
                .priority
                .current
                .lock()
                .unwrap()
                .iter()
                .flatten()
                .any(|s| s.client == [10; 16]));
            let reply = wait(connect_request(&port, [10; 16], "cancelled-client"));
            let Response::Connected { connection, .. } = reply.value() else {
                panic!()
            };
            let c = connection.clone();
            wait(port.source_capture(&c).unwrap());
            port.disconnect(&c).unwrap();
            wait(port.status().unwrap());
        }
        owner.shutdown();
    }
    #[test]
    #[ignore = "explicit pinned native owner connection-capability gate"]
    fn connection_capacity_reservation_fails_before_broker_registration() {
        let (owner, port, _, _, _, _, _, _) = setup();
        let (entered, resume) = pause(&port);
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        let pending = connect_request(&port, [11; 16], "pressure-client");
        let budget = port.memory_budget();
        // Permit the normal 64-KiB reply charge, but deny the subsequent 8-KiB
        // connection reservation, which must precede broker.negotiate.
        let pressure = budget
            .reserve(budget.limit() - budget.used() - (64 << 10) - 4096)
            .unwrap();
        resume.send(()).unwrap();
        assert!(pending.wait(Duration::from_secs(2)).is_err());
        assert!(!port
            .priority
            .current
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .any(|s| s.client == [11; 16]));
        drop(pressure);
        let reply = wait(connect_request(&port, [11; 16], "pressure-client"));
        let Response::Connected { connection, .. } = reply.value() else {
            panic!()
        };
        wait(port.source_capture(connection).unwrap());
        owner.shutdown();
    }

    #[test]
    #[ignore = "explicit pinned native owner local-device cancellation gate"]
    fn local_focus_cancel_discards_queued_old_context_and_neutralizes_actual_local_keys() {
        let (owner, port, local, c, _, _, _, _) = setup();
        let reply = wait(port.source_capture(&c).unwrap());
        let Response::Source(source) = reply.value() else {
            panic!()
        };
        let before = source.state().revision;
        let (entered, resume) = pause(&port);
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        local
            .native_event(LocalEvent::Key {
                code: 21,
                pressed: true,
            })
            .unwrap();
        local.cancel_input().unwrap();
        resume.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let reply = wait(port.status().unwrap());
            if matches!(reply.value(), Response::Status(OwnerStatus::Ready)) {
                break;
            }
            assert!(Instant::now() < deadline);
        }
        let reply = wait(port.source_capture(&c).unwrap());
        let Response::Source(source) = reply.value() else {
            panic!()
        };
        assert_eq!(source.state().revision, before);
        local
            .native_event(LocalEvent::Key {
                code: 42,
                pressed: true,
            })
            .unwrap();
        loop {
            let reply = wait(port.diagnostics().unwrap());
            let Response::Diagnostics(d) = reply.value() else {
                panic!()
            };
            if d.local_held_keys == 1 {
                break;
            }
            assert!(Instant::now() < deadline);
        }
        local.cancel_input().unwrap();
        loop {
            let reply = wait(port.diagnostics().unwrap());
            let Response::Diagnostics(d) = reply.value() else {
                panic!()
            };
            if d.local_held_keys == 0 {
                break;
            }
            assert!(Instant::now() < deadline);
        }
        local
            .native_event(LocalEvent::Key {
                code: 21,
                pressed: true,
            })
            .unwrap();
        local
            .native_event(LocalEvent::Key {
                code: 21,
                pressed: false,
            })
            .unwrap();
        loop {
            let reply = wait(port.source_capture(&c).unwrap());
            let Response::Source(source) = reply.value() else {
                panic!()
            };
            let value = source
                .state()
                .records
                .values()
                .find_map(|r| match r {
                    rui_protocol_core::model::Record::Semantic(s) => match &s.value {
                        Some(rui_protocol_core::model::TypedValue::Text(t)) => Some(t.as_str()),
                        _ => None,
                    },
                    _ => None,
                })
                .unwrap();
            if value == "z" {
                break;
            }
            assert!(Instant::now() < deadline);
        }
        owner.shutdown();
    }
    #[test]
    #[ignore = "explicit pinned native owner atomic resume-keeper gate"]
    fn connect_resumes_exact_published_checkpoint_even_after_authoritative_source_changes() {
        let (owner, port, local, old, _, revision, _, _) = setup();
        local
            .native_event(LocalEvent::Key {
                code: 21,
                pressed: true,
            })
            .unwrap();
        local
            .native_event(LocalEvent::Key {
                code: 21,
                pressed: false,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let reply = wait(port.diagnostics().unwrap());
            let Response::Diagnostics(d) = reply.value() else {
                panic!()
            };
            if d.local_held_keys == 0
                && port
                    .priority
                    .acknowledged_generation
                    .load(Ordering::Acquire)
                    == port.priority.generation.load(Ordering::Acquire)
            {
                break;
            }
            assert!(Instant::now() < deadline);
        }
        let reply = wait(port.source_capture(&old).unwrap());
        let Response::Source(source) = reply.value() else {
            panic!()
        };
        assert!(source.state().revision > revision);
        let reply = wait(connect_with_resume(
            &port,
            old.client().client_instance_id,
            &old.client().principal,
            Some(rui_protocol_core::session::ResumeRequest {
                epoch: old.epoch(),
                view_id: old.view(),
                acknowledged_revision: revision,
            }),
        ));
        let Response::Connected {
            connection,
            negotiated,
            resume_keeper,
        } = reply.value()
        else {
            panic!()
        };
        assert_eq!(connection.view(), old.view());
        assert!(
            matches!(negotiated.resume,ResumeDisposition::Accepted {from_revision,through_revision,..} if from_revision==revision && through_revision==revision)
        );
        let envelope = pb::Envelope::decode(resume_keeper.as_ref().unwrap().as_ref()).unwrap();
        let snapshot = pb::Snapshot::decode(envelope.payload.as_slice()).unwrap();
        assert_eq!(snapshot.revision, revision);
        let checkpoint = rui_protocol_core::model::State::from_source_records(
            revision,
            snapshot.records,
            &rui_protocol_core::model::Limits::default(),
        )
        .unwrap();
        let text = checkpoint
            .records
            .values()
            .find_map(|r| match r {
                rui_protocol_core::model::Record::Semantic(s) => match &s.value {
                    Some(rui_protocol_core::model::TypedValue::Text(t)) => Some(t.as_str()),
                    _ => None,
                },
                _ => None,
            })
            .unwrap();
        assert_eq!(text, "");
        assert!(port.source_capture(&old).is_err());
        assert!(
            port.acquire_control(connection, [31; 16])
                .unwrap()
                .wait(Duration::from_secs(2))
                .is_err(),
            "resume must not restore presentation/control"
        );
        owner.shutdown();
    }
}

#[cfg(test)]
mod reply_notifications {
    use super::*;
    use std::{
        future::Future,
        task::{Context, Poll, Wake, Waker},
    };
    struct Signal(AtomicBool, ReplyWake);
    impl Wake for Signal {
        fn wake(self: Arc<Self>) {
            assert!(self.1.try_lock().is_ok(), "waker invoked under reply lock");
            self.0.store(true, Ordering::Release);
        }
    }
    #[test]
    fn replies_and_sender_loss_wake_pending_receivers() {
        for send in [false, true] {
            let (sender, receiver) = mpsc::sync_channel(1);
            let wake = Arc::new(Mutex::new(None));
            let sender = ReplySender(Some(sender), wake.clone());
            let signal = Arc::new(Signal(AtomicBool::new(false), wake.clone()));
            let waker = Waker::from(signal.clone());
            let mut context = Context::from_waker(&waker);
            let mut future = Box::pin(Pending(receiver, wake).receive());
            assert!(future.as_mut().poll(&mut context).is_pending());
            if send {
                assert!(sender.send(Err(ShellError::Limit)).is_none());
            } else {
                drop(sender);
            }
            assert!(signal.0.load(Ordering::Acquire));
            assert!(matches!(
                future.as_mut().poll(&mut context),
                Poll::Ready(Err(_))
            ));
        }
    }
}
