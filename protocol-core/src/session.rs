//! Transport-independent authenticated negotiation, bounded recovery and one
//! shared-seat control lease. Cleanup records describe work for a host adapter;
//! this module does not inject OS input, authenticate credentials, or serve TLS.
use crate::commands::{
    AuthenticatedClient, CommandFamily, CommandHeader, Id, Rejection, SemanticCommand,
};
use std::any::Any;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ops::Deref;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Weak,
};
use thiserror::Error;

/// Conservative allocation allowance for the payload Arc, reservation and both
/// queue/history entries, including spare collection capacity. Byte ceilings
/// charge this even for empty publications; record ceilings provide a second
/// independent bound on entry counts.
pub const PUBLICATION_OVERHEAD_BYTES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiverLimits {
    pub envelope_bytes: usize,
    pub chunk_bytes: usize,
    pub transaction_bytes: usize,
    pub snapshot_bytes: usize,
    pub resource_bytes: usize,
    pub view_memory_bytes: usize,
    pub outbound_queue_bytes: usize,
    pub records: usize,
    pub ownership_depth: usize,
}
impl Default for ReceiverLimits {
    fn default() -> Self {
        Self {
            envelope_bytes: 1024 * 1024,
            chunk_bytes: 64 * 1024,
            transaction_bytes: 16 * 1024 * 1024,
            snapshot_bytes: 64 * 1024 * 1024,
            resource_bytes: 16 * 1024 * 1024,
            view_memory_bytes: 128 * 1024 * 1024,
            outbound_queue_bytes: 8 * 1024 * 1024,
            records: 100_000,
            ownership_depth: 128,
        }
    }
}
impl ReceiverLimits {
    pub fn negotiate(&self, peer: &Self) -> Result<Self, SessionError> {
        let limits = Self {
            envelope_bytes: self.envelope_bytes.min(peer.envelope_bytes),
            chunk_bytes: self.chunk_bytes.min(peer.chunk_bytes),
            transaction_bytes: self.transaction_bytes.min(peer.transaction_bytes),
            snapshot_bytes: self.snapshot_bytes.min(peer.snapshot_bytes),
            resource_bytes: self.resource_bytes.min(peer.resource_bytes),
            view_memory_bytes: self.view_memory_bytes.min(peer.view_memory_bytes),
            outbound_queue_bytes: self.outbound_queue_bytes.min(peer.outbound_queue_bytes),
            records: self.records.min(peer.records),
            ownership_depth: self.ownership_depth.min(peer.ownership_depth),
        };
        if [
            limits.envelope_bytes,
            limits.chunk_bytes,
            limits.transaction_bytes,
            limits.snapshot_bytes,
            limits.resource_bytes,
            limits.view_memory_bytes,
            limits.outbound_queue_bytes,
            limits.records,
            limits.ownership_depth,
        ]
        .contains(&0)
        {
            return Err(SessionError::InvalidLimits);
        }
        Ok(limits)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Permissions {
    pub can_view: bool,
    pub can_control: bool,
    pub actions: BTreeSet<String>,
}
/// Supplied only after the trusted authentication and authorization layers have
/// verified credentials and computed policy. A client-declared principal is not
/// a valid source for this value.
#[derive(Clone, Debug)]
pub struct VerifiedAuthorization {
    pub principal: String,
    pub policy_version: u64,
    pub permissions: Permissions,
}
#[derive(Clone, Debug)]
pub struct ClientHello {
    pub client_instance_id: Id,
    pub supported_versions: BTreeSet<u32>,
    pub supported_profiles: BTreeSet<String>,
    pub required_profiles: BTreeSet<String>,
    pub receiver_limits: ReceiverLimits,
    pub resume: Option<ResumeRequest>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeRequest {
    pub epoch: Id,
    pub view_id: Id,
    pub acknowledged_revision: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResetReason {
    EpochChanged,
    ViewChanged,
    NoUsableAcknowledgement,
    HistoryUnavailable,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeDisposition {
    FreshSnapshot,
    Accepted {
        from_revision: u64,
        through_revision: u64,
        transactions: Vec<SharedPayload>,
        requires_resources: bool,
    },
    ResetRequired(ResetReason),
}
#[derive(Clone, Debug)]
pub struct NegotiatedSession {
    pub session_id: Id,
    pub epoch: Id,
    pub client: AuthenticatedClient,
    pub version: u32,
    pub profiles: BTreeSet<String>,
    pub limits: ReceiverLimits,
    pub view_id: Id,
    pub permissions: Permissions,
    pub resume: ResumeDisposition,
}
#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub versions: BTreeSet<u32>,
    pub supported_profiles: BTreeSet<String>,
    pub required_profiles: BTreeSet<String>,
    pub receiver_limits: ReceiverLimits,
    pub max_clients: usize,
    pub max_history_records: usize,
    pub max_history_bytes: usize,
    pub max_control_requests: usize,
    pub max_held_inputs: usize,
    pub lease_duration_ms: u64,
}
impl Default for SessionConfig {
    fn default() -> Self {
        let profiles: BTreeSet<String> = [
            "mirror2d.core.v1",
            "semantics.core.v1",
            "content.no-raster.v1",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        Self {
            versions: [1].into_iter().collect(),
            supported_profiles: profiles.clone(),
            required_profiles: profiles,
            receiver_limits: ReceiverLimits::default(),
            max_clients: 1024,
            max_history_records: 256,
            max_history_bytes: 16 * 1024 * 1024,
            max_control_requests: 65_536,
            max_held_inputs: 1024,
            lease_duration_ms: 5000,
        }
    }
}
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SessionError {
    #[error("authentication is required before negotiation and projection")]
    Unauthenticated,
    #[error("principal or registered client binding does not match")]
    WrongPrincipal,
    #[error("view is not authorized")]
    Unauthorized,
    #[error("no common protocol version")]
    UnsupportedVersion,
    #[error("required profile is unsupported")]
    UnsupportedProfile,
    #[error("invalid receiver or broker limits")]
    InvalidLimits,
    #[error("bounded epoch retention budget exhausted")]
    RetentionExhausted,
    #[error("view is obsolete or closed")]
    WrongView,
    #[error("the client is disconnected")]
    Disconnected,
    #[error("another remote client controls the shared seat")]
    ControlBusy,
    #[error("lease or generation is stale")]
    StaleControl,
    #[error("a control request ID was reused with different parameters")]
    ChangedRequest,
    #[error("an expired or revoked request cannot resurrect a grant")]
    StaleRequest,
    #[error("monotonic clock or generation space exhausted")]
    ClockExhausted,
    #[error("publication does not extend the current view revision")]
    WrongRevision,
    #[error("payload exceeds negotiated receiver ceilings")]
    Oversized,
    #[error("slow consumer closed; dependent publications were not dropped")]
    SlowConsumer,
    #[error("no usable state and resources are ready for presentation")]
    NotPresentable,
}
#[derive(Debug)]
struct MemoryBudget {
    used: AtomicUsize,
    limit: AtomicUsize,
}
#[derive(Debug)]
struct Reservation {
    budget: Arc<MemoryBudget>,
    bytes: usize,
}
/// Opaque trusted-adapter allocation charge against this authenticated view.
/// Acquire it before allocating captures, work buffers or diagnostic state and
/// retain it for the entire allocation lifetime. It cannot be cloned.
pub struct ViewMemoryReservation {
    view_id: Id,
    _reservation: Reservation,
}
/// An opaque, shared trusted-adapter allocation. The reservation must cover the
/// value and every allocation it owns before construction. Private adapter types
/// prevent callers from extracting uncharged owned content. Publication/history
/// clones retain this charge until the last handle drops.
#[derive(Clone)]
pub struct PublicationAttachment(Arc<Attachment>);
struct Attachment {
    memory: ViewMemoryReservation,
    value: Box<dyn Any + Send + Sync>,
}
pub struct WeakPublicationAttachment(Weak<Attachment>);
impl PublicationAttachment {
    pub fn get<T: Any>(&self) -> Option<&T> {
        self.0.value.downcast_ref()
    }
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn downgrade(&self) -> WeakPublicationAttachment {
        WeakPublicationAttachment(Arc::downgrade(&self.0))
    }
}
impl WeakPublicationAttachment {
    pub fn upgrade(&self) -> Option<PublicationAttachment> {
        self.0.upgrade().map(PublicationAttachment)
    }
}
impl ViewMemoryReservation {
    pub fn attach<T: Any + Send + Sync>(self, value: T) -> PublicationAttachment {
        PublicationAttachment(Arc::new(Attachment {
            memory: self,
            value: Box::new(value),
        }))
    }
}
impl Reservation {
    fn new(budget: Arc<MemoryBudget>, bytes: usize) -> Result<Self, SessionError> {
        budget
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|next| *next <= budget.limit.load(Ordering::Acquire))
            })
            .map_err(|_| SessionError::Oversized)?;
        Ok(Self { budget, bytes })
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
struct Payload {
    view_id: Id,
    publication_revision: Option<u64>,
    checkpoint_generation: u64,
    bytes: Box<[u8]>,
    _reservation: Reservation,
    _attachment: Option<PublicationAttachment>,
}
/// Shared publication bytes remain charged to this producer's memory budget
/// until the last history, queue, outbound or resume reference is dropped.
#[derive(Clone)]
pub struct SharedPayload(Arc<Payload>);
impl std::fmt::Debug for SharedPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedPayload")
            .field("bytes", &self.len())
            .field("attachment", &self.0._attachment.is_some())
            .finish()
    }
}
impl SharedPayload {
    /// Broker-stamped transaction revision; arbitrary snapshot byte handles
    /// are never retained-transaction acknowledgement proofs.
    pub fn publication_revision(&self) -> Option<u64> {
        self.0.publication_revision
    }
    /// Borrow the trusted adapter keepalive; content access still requires the
    /// adapter's current authenticated scope check.
    pub fn attachment(&self) -> Option<&PublicationAttachment> {
        self.0._attachment.as_ref()
    }
}
impl Deref for SharedPayload {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0.bytes
    }
}
impl PartialEq for SharedPayload {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}
impl Eq for SharedPayload {}
#[derive(Clone)]
struct HistoryEntry {
    base: u64,
    revision: u64,
    bytes: SharedPayload,
}
struct Connection {
    identity: AuthenticatedClient,
    authorization: VerifiedAuthorization,
    version: u32,
    profiles: BTreeSet<String>,
    limits: ReceiverLimits,
    view_id: Id,
    connected: bool,
    closed: bool,
    revision: Option<u64>,
    snapshot_revision: Option<u64>,
    checkpoint_generation: u64,
    acknowledged_revision: Option<u64>,
    resources_ready: bool,
    presented_revision: Option<u64>,
    semantic_only: bool,
    semantic_ready_revision: Option<u64>,
    budget: Arc<MemoryBudget>,
    snapshot_allocation: Option<Reservation>,
    history: VecDeque<HistoryEntry>,
    history_bytes: usize,
    queue: VecDeque<SharedPayload>,
    queue_bytes: usize,
}

impl Connection {
    fn input_ready(&self, allow_semantic: bool) -> bool {
        self.authorization.permissions.can_control
            && self.acknowledged_revision.is_some()
            && self.resources_ready
            && if self.semantic_only {
                allow_semantic && self.semantic_ready_revision.is_some()
            } else {
                self.presented_revision.is_some()
            }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlLease {
    pub lease_id: Id,
    pub generation: u64,
    pub client: AuthenticatedClient,
    pub view_id: Id,
    pub expires_at_ms: u64,
    pub allowed_actions: BTreeSet<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysicalKey {
    pub namespace: String,
    pub code: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CleanupReason {
    Disconnect,
    Expired,
    LocalActivity,
    Released,
    AuthorizationChanged,
    SlowConsumer,
    ExplicitRevocation,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputCleanup {
    pub lease: ControlLease,
    pub reason: CleanupReason,
    pub keys: Vec<PhysicalKey>,
    pub buttons: Vec<u32>,
    pub touches: Vec<u64>,
    pub composition: Option<Id>,
}
struct ActiveControl {
    lease: ControlLease,
    keys: BTreeSet<PhysicalKey>,
    buttons: BTreeSet<u32>,
    touches: BTreeSet<u64>,
    composition: Option<Id>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum RequestPayload {
    Acquire { view: Id },
    Renew { lease: Id, generation: u64 },
    Release { lease: Id, generation: u64 },
}
#[derive(Clone)]
enum RequestResult {
    Grant(ControlLease),
    Released,
}
struct RequestRecord {
    payload: RequestPayload,
    result: RequestResult,
}

pub struct SessionBroker {
    session_id: Id,
    epoch: Id,
    config: SessionConfig,
    connections: BTreeMap<Id, Connection>,
    next_view: u64,
    generation: u64,
    last_now: u64,
    active: Option<ActiveControl>,
    requests: BTreeMap<(AuthenticatedClient, Id), RequestRecord>,
    cleanup: VecDeque<InputCleanup>,
}
impl SessionBroker {
    pub fn new(session_id: Id, epoch: Id, config: SessionConfig) -> Result<Self, SessionError> {
        config.receiver_limits.negotiate(&config.receiver_limits)?;
        if config.max_clients == 0
            || config.max_history_records == 0
            || config.max_history_bytes == 0
            || config.max_control_requests == 0
            || config.max_held_inputs == 0
            || config.lease_duration_ms == 0
            || config.versions.is_empty()
            || !config
                .required_profiles
                .is_subset(&config.supported_profiles)
        {
            return Err(SessionError::InvalidLimits);
        }
        Ok(Self {
            session_id,
            epoch,
            config,
            connections: BTreeMap::new(),
            next_view: 0,
            generation: 0,
            last_now: 0,
            active: None,
            requests: BTreeMap::new(),
            cleanup: VecDeque::new(),
        })
    }
    // Identifiers are unique in this authority epoch; they are never credentials.
    fn identifier(prefix: Id, counter: u64) -> Id {
        let mut id = prefix;
        id[8..].copy_from_slice(&counter.to_be_bytes());
        id
    }
    fn new_view_id(&mut self) -> Result<Id, SessionError> {
        self.next_view = self
            .next_view
            .checked_add(1)
            .ok_or(SessionError::ClockExhausted)?;
        Ok(Self::identifier(self.session_id, self.next_view))
    }
    fn observe_time(&mut self, now: u64) -> u64 {
        self.last_now = self.last_now.max(now);
        self.last_now
    }
    fn revoke_active(&mut self, reason: CleanupReason) {
        if let Some(active) = self.active.take() {
            self.cleanup.push_back(InputCleanup {
                lease: active.lease,
                reason,
                keys: active.keys.into_iter().collect(),
                buttons: active.buttons.into_iter().collect(),
                touches: active.touches.into_iter().collect(),
                composition: active.composition,
            });
        }
    }
    pub fn tick(&mut self, now: u64) {
        let now = self.observe_time(now);
        if self
            .active
            .as_ref()
            .is_some_and(|a| a.lease.expires_at_ms <= now)
        {
            self.revoke_active(CleanupReason::Expired);
        }
    }
    /// Drain promptly into the adapter. New grants are refused while cleanup is
    /// pending, bounding the queue and ensuring the old seat is unwound first.
    pub fn take_cleanup(&mut self) -> Option<InputCleanup> {
        self.cleanup.pop_front()
    }
    pub fn local_activity(&mut self, now: u64) {
        self.tick(now);
        self.revoke_active(CleanupReason::LocalActivity);
    }
    pub fn revoke_control(&mut self, now: u64) {
        self.tick(now);
        self.revoke_active(CleanupReason::ExplicitRevocation);
    }

    /// Authentication and policy checks precede creating any view projection.
    /// Every new connection drops its former lease and transient input.
    pub fn negotiate(
        &mut self,
        authorization: Option<VerifiedAuthorization>,
        hello: ClientHello,
        now: u64,
    ) -> Result<NegotiatedSession, SessionError> {
        let auth = authorization.ok_or(SessionError::Unauthenticated)?;
        if auth.principal.is_empty() || auth.principal.len() > 1024 {
            return Err(SessionError::Unauthenticated);
        }
        if !auth.permissions.can_view {
            return Err(SessionError::Unauthorized);
        }
        if hello.supported_profiles.len() > 128
            || hello.required_profiles.len() > 128
            || hello.supported_versions.len() > 128
            || auth.permissions.actions.len() > 128
            || hello
                .supported_profiles
                .iter()
                .chain(hello.required_profiles.iter())
                .chain(auth.permissions.actions.iter())
                .any(|p| p.is_empty() || p.len() > 256)
        {
            return Err(SessionError::Oversized);
        }
        let version = self
            .config
            .versions
            .intersection(&hello.supported_versions)
            .last()
            .copied()
            .ok_or(SessionError::UnsupportedVersion)?;
        let profiles: BTreeSet<_> = self
            .config
            .supported_profiles
            .intersection(&hello.supported_profiles)
            .cloned()
            .collect();
        if !hello.required_profiles.is_subset(&profiles)
            || !self.config.required_profiles.is_subset(&profiles)
        {
            return Err(SessionError::UnsupportedProfile);
        }
        let limits = self
            .config
            .receiver_limits
            .negotiate(&hello.receiver_limits)?;
        let client = AuthenticatedClient {
            principal: auth.principal.clone(),
            client_instance_id: hello.client_instance_id,
        };
        if !self.connections.contains_key(&hello.client_instance_id)
            && self.connections.len() >= self.config.max_clients
        {
            return Err(SessionError::RetentionExhausted);
        }
        // An attacker cannot take over a producer by declaring its client ID.
        if self
            .connections
            .get(&hello.client_instance_id)
            .is_some_and(|old| old.identity.principal != client.principal)
        {
            return Err(SessionError::WrongPrincipal);
        }
        // Returned payloads can outlive history and connections. Do not claim a
        // newly lowered ceiling until every allocation still charged to this
        // producer fits. This preflight leaves its former limits and view intact
        // on failure; the caller may drain outstanding work and retry.
        if self
            .connections
            .get(&hello.client_instance_id)
            .is_some_and(|old| old.budget.used.load(Ordering::Acquire) > limits.view_memory_bytes)
        {
            return Err(SessionError::Oversized);
        }
        self.tick(now);
        if self
            .active
            .as_ref()
            .is_some_and(|a| a.lease.client.client_instance_id == hello.client_instance_id)
        {
            self.revoke_active(CleanupReason::Disconnect);
        }
        let unchanged = self
            .connections
            .get(&hello.client_instance_id)
            .is_some_and(|old| {
                old.identity == client
                    && old.authorization.policy_version == auth.policy_version
                    && old.authorization.permissions == auth.permissions
                    && old.profiles == profiles
                    && old.version == version
                    && !old.closed
            });
        if !unchanged {
            let view_id = self.new_view_id()?;
            let budget = self
                .connections
                .get(&hello.client_instance_id)
                .map(|old| old.budget.clone())
                .unwrap_or_else(|| {
                    Arc::new(MemoryBudget {
                        used: AtomicUsize::new(0),
                        limit: AtomicUsize::new(limits.view_memory_bytes),
                    })
                });
            budget
                .limit
                .store(limits.view_memory_bytes, Ordering::Release);
            self.connections.insert(
                hello.client_instance_id,
                Connection {
                    identity: client.clone(),
                    authorization: auth.clone(),
                    version,
                    profiles: profiles.clone(),
                    limits: limits.clone(),
                    view_id,
                    connected: true,
                    closed: false,
                    revision: None,
                    snapshot_revision: None,
                    checkpoint_generation: 0,
                    acknowledged_revision: None,
                    resources_ready: false,
                    presented_revision: None,
                    semantic_only: profiles.contains("semantics.only.v1"),
                    semantic_ready_revision: None,
                    budget,
                    snapshot_allocation: None,
                    history: VecDeque::new(),
                    history_bytes: 0,
                    queue: VecDeque::new(),
                    queue_bytes: 0,
                },
            );
        } else {
            let old = self.connections.get_mut(&hello.client_instance_id).unwrap();
            old.connected = true;
            old.limits = limits.clone();
            old.budget
                .limit
                .store(limits.view_memory_bytes, Ordering::Release);
            old.presented_revision = None;
            old.semantic_ready_revision = None;
            old.queue.clear();
            old.queue_bytes = 0;
            // Previously retained history may exceed newly lowered limits.
            let max = self.config.max_history_bytes.min(limits.view_memory_bytes);
            while old.history_bytes > max
                || old.budget.used.load(Ordering::Acquire) > limits.view_memory_bytes
            {
                if let Some(entry) = old.history.pop_front() {
                    old.history_bytes -= entry.bytes.len();
                } else {
                    break;
                }
            }
        }
        let conn = self.connections.get(&hello.client_instance_id).unwrap();
        let resume = hello
            .resume
            .as_ref()
            .map_or(ResumeDisposition::FreshSnapshot, |r| {
                self.resume_disposition(conn, r)
            });
        if !matches!(resume, ResumeDisposition::Accepted { .. }) {
            let conn = self.connections.get_mut(&hello.client_instance_id).unwrap();
            conn.acknowledged_revision = None;
            conn.resources_ready = false;
        }
        let conn = self.connections.get(&hello.client_instance_id).unwrap();
        Ok(NegotiatedSession {
            session_id: self.session_id,
            epoch: self.epoch,
            client,
            version,
            profiles,
            limits,
            view_id: conn.view_id,
            permissions: auth.permissions,
            resume,
        })
    }
    fn resume_disposition(&self, conn: &Connection, request: &ResumeRequest) -> ResumeDisposition {
        let reset = |r| ResumeDisposition::ResetRequired(r);
        if request.epoch != self.epoch {
            return reset(ResetReason::EpochChanged);
        }
        if request.view_id != conn.view_id {
            return reset(ResetReason::ViewChanged);
        }
        if conn.acknowledged_revision != Some(request.acknowledged_revision)
            || conn.revision.is_none()
            || conn.budget.used.load(Ordering::Acquire) > conn.limits.view_memory_bytes
        {
            return reset(ResetReason::NoUsableAcknowledgement);
        }
        let latest = conn.revision.unwrap();
        let mut base = request.acknowledged_revision;
        let mut transactions = Vec::new();
        for entry in &conn.history {
            if entry.revision <= base {
                continue;
            }
            if entry.base != base || entry.bytes.len() > conn.limits.transaction_bytes {
                return reset(ResetReason::HistoryUnavailable);
            }
            transactions.push(entry.bytes.clone());
            base = entry.revision;
        }
        if base != latest {
            return reset(ResetReason::HistoryUnavailable);
        }
        ResumeDisposition::Accepted {
            from_revision: request.acknowledged_revision,
            through_revision: latest,
            transactions,
            requires_resources: !conn.resources_ready,
        }
    }
    // Trusted producer retention may continue for a disconnected registered
    // view. It conveys neither a readable projection nor any input authority.
    fn producer_connection(
        &self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<&Connection, SessionError> {
        let conn = self
            .connections
            .get(&client.client_instance_id)
            .ok_or(SessionError::Unauthenticated)?;
        if conn.identity != *client {
            return Err(SessionError::WrongPrincipal);
        }
        if conn.closed || conn.view_id != view {
            return Err(SessionError::WrongView);
        }
        if !conn.authorization.permissions.can_view {
            return Err(SessionError::Unauthorized);
        }
        Ok(conn)
    }
    fn connection(
        &self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<&Connection, SessionError> {
        let conn = self
            .connections
            .get(&client.client_instance_id)
            .ok_or(SessionError::Unauthenticated)?;
        if conn.identity != *client {
            return Err(SessionError::WrongPrincipal);
        }
        if conn.closed || conn.view_id != view {
            return Err(SessionError::WrongView);
        }
        if !conn.connected {
            return Err(SessionError::Disconnected);
        }
        if !conn.authorization.permissions.can_view {
            return Err(SessionError::Unauthorized);
        }
        Ok(conn)
    }
    fn connection_mut(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<&mut Connection, SessionError> {
        self.connection(client, view)?;
        Ok(self
            .connections
            .get_mut(&client.client_instance_id)
            .unwrap())
    }
    pub fn authorize_projection(
        &self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<&Permissions, SessionError> {
        Ok(&self.connection(client, view)?.authorization.permissions)
    }
    pub fn disconnect(
        &mut self,
        client: &AuthenticatedClient,
        now: u64,
    ) -> Result<(), SessionError> {
        self.tick(now);
        let conn = self
            .connections
            .get_mut(&client.client_instance_id)
            .ok_or(SessionError::Unauthenticated)?;
        if conn.identity != *client {
            return Err(SessionError::WrongPrincipal);
        }
        conn.connected = false;
        conn.queue.clear();
        conn.queue_bytes = 0;
        if self
            .active
            .as_ref()
            .is_some_and(|a| a.lease.client == *client)
        {
            self.revoke_active(CleanupReason::Disconnect);
        }
        Ok(())
    }
    /// Atomically invalidate an existing authorization, projection, history and
    /// resource-lookup permission. A subsequent negotiation creates a new view.
    pub fn invalidate_authorization(
        &mut self,
        client: &AuthenticatedClient,
        now: u64,
    ) -> Result<(), SessionError> {
        self.tick(now);
        let conn = self
            .connections
            .get_mut(&client.client_instance_id)
            .ok_or(SessionError::Unauthenticated)?;
        if conn.identity != *client {
            return Err(SessionError::WrongPrincipal);
        }
        conn.closed = true;
        conn.history.clear();
        conn.history_bytes = 0;
        conn.queue.clear();
        conn.queue_bytes = 0;
        conn.revision = None;
        conn.acknowledged_revision = None;
        conn.resources_ready = false;
        conn.presented_revision = None;
        conn.semantic_ready_revision = None;
        conn.snapshot_allocation = None;
        if self
            .active
            .as_ref()
            .is_some_and(|a| a.lease.client == *client)
        {
            self.revoke_active(CleanupReason::AuthorizationChanged);
        }
        Ok(())
    }
    pub fn install_snapshot(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        revision: u64,
        decoded_bytes: usize,
    ) -> Result<(), SessionError> {
        let conn = self.connection_mut(client, view)?;
        if revision == 0 || conn.revision.is_some_and(|current| revision < current) {
            return Err(SessionError::WrongRevision);
        }
        if decoded_bytes > conn.limits.snapshot_bytes
            || decoded_bytes > conn.limits.view_memory_bytes
        {
            return Err(SessionError::Oversized);
        }
        let checkpoint_generation = conn
            .checkpoint_generation
            .checked_add(1)
            .ok_or(SessionError::WrongRevision)?;
        let allocation = Reservation::new(conn.budget.clone(), decoded_bytes)?;
        conn.checkpoint_generation = checkpoint_generation;
        conn.snapshot_allocation = Some(allocation);
        conn.revision = Some(revision);
        conn.snapshot_revision = Some(revision);
        conn.acknowledged_revision = None;
        conn.presented_revision = None;
        conn.semantic_ready_revision = None;
        conn.resources_ready = false;
        conn.history.clear();
        conn.history_bytes = 0;
        conn.queue.clear();
        conn.queue_bytes = 0;
        Ok(())
    }
    /// Publish one whole dependent payload. A slow client's view closes explicitly
    /// before any subsequent payload can be queued; other clients are unaffected.
    pub fn publish(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        base: u64,
        revision: u64,
        bytes: Vec<u8>,
        now: u64,
    ) -> Result<(), SessionError> {
        self.publish_with_attachment(client, view, base, revision, bytes, None, now)
    }
    /// Trusted producer publication whose source-resource closure shares this
    /// view budget. Attachment handles from another view are rejected.
    #[allow(clippy::too_many_arguments)]
    pub fn publish_with_attachment(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        base: u64,
        revision: u64,
        bytes: Vec<u8>,
        attachment: Option<PublicationAttachment>,
        now: u64,
    ) -> Result<(), SessionError> {
        self.connection(client, view)?;
        self.publish_retained_with_attachment(client, view, base, revision, bytes, attachment, now)
    }
    /// Trusted source publication for the current registered policy even while
    /// disconnected. The authentication adapter must invalidate revoked grants;
    /// this method never permits input or projection reads. Disconnected updates
    /// retain bounded history only, without growing an outbound socket queue.
    #[allow(clippy::too_many_arguments)]
    pub fn publish_retained_with_attachment(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        base: u64,
        revision: u64,
        bytes: Vec<u8>,
        attachment: Option<PublicationAttachment>,
        now: u64,
    ) -> Result<(), SessionError> {
        self.tick(now);
        let max_history_records = self.config.max_history_records;
        let max_history_bytes = self.config.max_history_bytes;
        self.producer_connection(client, view)?;
        let conn = self
            .connections
            .get_mut(&client.client_instance_id)
            .unwrap();
        if attachment.as_ref().is_some_and(|a| {
            a.0.memory.view_id != view
                || !Arc::ptr_eq(&a.0.memory._reservation.budget, &conn.budget)
        }) {
            return Err(SessionError::WrongView);
        }
        if conn.revision != Some(base) || base.checked_add(1) != Some(revision) {
            return Err(SessionError::WrongRevision);
        }
        if bytes.len() > conn.limits.transaction_bytes
            || bytes.len() > conn.limits.view_memory_bytes
        {
            return Err(SessionError::Oversized);
        }
        let reservation = bytes
            .len()
            .checked_add(PUBLICATION_OVERHEAD_BYTES)
            .ok_or(SessionError::Oversized)
            .and_then(|charge| Reservation::new(conn.budget.clone(), charge));
        if (conn.connected
            && (conn
                .queue_bytes
                .checked_add(bytes.len())
                .is_none_or(|n| n > conn.limits.outbound_queue_bytes)
                || conn.queue.len() >= conn.limits.records))
            || reservation.is_err()
        {
            conn.closed = true;
            conn.queue.clear();
            conn.queue_bytes = 0;
            conn.history.clear();
            conn.history_bytes = 0;
            if self
                .active
                .as_ref()
                .is_some_and(|a| a.lease.client == *client)
            {
                self.revoke_active(CleanupReason::SlowConsumer);
            }
            return Err(SessionError::SlowConsumer);
        }
        let bytes = SharedPayload(Arc::new(Payload {
            view_id: view,
            publication_revision: Some(revision),
            checkpoint_generation: conn.checkpoint_generation,
            bytes: bytes.into_boxed_slice(),
            _reservation: reservation.unwrap(),
            _attachment: attachment,
        }));
        if conn.connected {
            conn.queue_bytes += bytes.len();
            conn.queue.push_back(bytes.clone());
        }
        conn.history_bytes += bytes.len();
        conn.history.push_back(HistoryEntry {
            base,
            revision,
            bytes,
        });
        conn.revision = Some(revision);
        while conn.history.len() > max_history_records.min(conn.limits.records)
            || conn.history_bytes > max_history_bytes.min(conn.limits.view_memory_bytes)
        {
            if let Some(entry) = conn.history.pop_front() {
                conn.history_bytes -= entry.bytes.len();
            }
        }
        Ok(())
    }
    pub fn take_outbound(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<Option<SharedPayload>, SessionError> {
        let conn = self.connection_mut(client, view)?;
        let bytes = conn.queue.pop_front();
        if let Some(bytes) = &bytes {
            conn.queue_bytes -= bytes.len();
        }
        Ok(bytes)
    }
    /// Reserve a read-only snapshot envelope without resetting history, state or
    /// acknowledgements. The caller separately installs an explicit checkpoint
    /// when needed. Every returned clone remains charged until its last drop.
    pub fn snapshot_payload(
        &self,
        client: &AuthenticatedClient,
        view: Id,
        bytes: Vec<u8>,
    ) -> Result<SharedPayload, SessionError> {
        let conn = self.connection(client, view)?;
        if bytes.len() > conn.limits.envelope_bytes {
            return Err(SessionError::Oversized);
        }
        self.snapshot_publication_payload(client, view, bytes, None)
    }
    /// Whole snapshot producer payload for bounded begin/chunk/commit delivery.
    /// It may exceed a single envelope limit; transports must chunk it, and must
    /// still enforce the envelope limit on every actual socket message.
    pub fn snapshot_publication_payload(
        &self,
        client: &AuthenticatedClient,
        view: Id,
        bytes: Vec<u8>,
        attachment: Option<PublicationAttachment>,
    ) -> Result<SharedPayload, SessionError> {
        let conn = self.connection(client, view)?;
        if attachment.as_ref().is_some_and(|a| {
            a.0.memory.view_id != view
                || !Arc::ptr_eq(&a.0.memory._reservation.budget, &conn.budget)
        }) {
            return Err(SessionError::WrongView);
        }
        if bytes.len() > conn.limits.snapshot_bytes {
            return Err(SessionError::Oversized);
        }
        let charge = bytes
            .len()
            .checked_add(PUBLICATION_OVERHEAD_BYTES)
            .ok_or(SessionError::Oversized)?;
        let reservation = Reservation::new(conn.budget.clone(), charge)?;
        Ok(SharedPayload(Arc::new(Payload {
            view_id: view,
            publication_revision: None,
            checkpoint_generation: conn.checkpoint_generation,
            bytes: bytes.into_boxed_slice(),
            _reservation: reservation,
            _attachment: attachment,
        })))
    }
    pub fn queued_bytes(
        &self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<usize, SessionError> {
        Ok(self.connection(client, view)?.queue_bytes)
    }
    pub fn retained_memory_bytes(
        &self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<usize, SessionError> {
        Ok(self
            .connection(client, view)?
            .budget
            .used
            .load(Ordering::Acquire))
    }
    /// Share the same ceiling used by decoded snapshots, history, queues and
    /// returned payloads with trusted source/presentation adapter allocations.
    pub fn reserve_view_memory(
        &self,
        client: &AuthenticatedClient,
        view: Id,
        bytes: usize,
    ) -> Result<ViewMemoryReservation, SessionError> {
        let conn = self.connection(client, view)?;
        Ok(ViewMemoryReservation {
            view_id: view,
            _reservation: Reservation::new(conn.budget.clone(), bytes)?,
        })
    }
    /// Trusted source scratch/closure retention under the current registered
    /// policy; unlike projection reads this permits a disconnected view.
    pub fn reserve_producer_view_memory(
        &self,
        client: &AuthenticatedClient,
        view: Id,
        bytes: usize,
    ) -> Result<ViewMemoryReservation, SessionError> {
        let conn = self.producer_connection(client, view)?;
        Ok(ViewMemoryReservation {
            view_id: view,
            _reservation: Reservation::new(conn.budget.clone(), bytes)?,
        })
    }
    /// Recheck an adapter allocation against this currently authenticated view.
    /// Both view identity and the broker's private budget identity must match.
    pub fn authorize_attachment(
        &self,
        client: &AuthenticatedClient,
        view: Id,
        attachment: &PublicationAttachment,
    ) -> Result<(), SessionError> {
        let conn = self.connection(client, view)?;
        if attachment.0.memory.view_id != view
            || !Arc::ptr_eq(&attachment.0.memory._reservation.budget, &conn.budget)
        {
            return Err(SessionError::WrongView);
        }
        Ok(())
    }
    pub fn acknowledge_applied(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        revision: u64,
        resources_ready: bool,
    ) -> Result<(), SessionError> {
        self.acknowledge_applied_inner(client, view, revision, resources_ready, false)
    }
    /// Trusted sender proof for a broker-published transaction retained through
    /// delivery even after recovery history evicts it. An arbitrary snapshot
    /// payload or attachment cannot create this proof. The current authenticated
    /// view, budget, checkpoint generation and exact stamped revision must match.
    pub fn acknowledge_applied_publication(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        revision: u64,
        resources_ready: bool,
        publication: &SharedPayload,
    ) -> Result<(), SessionError> {
        let conn = self.connection(client, view)?;
        if publication.0.view_id != view
            || !Arc::ptr_eq(&publication.0._reservation.budget, &conn.budget)
        {
            return Err(SessionError::WrongView);
        }
        if publication.0.publication_revision != Some(revision)
            || publication.0.checkpoint_generation != conn.checkpoint_generation
        {
            return Err(SessionError::WrongRevision);
        }
        self.acknowledge_applied_inner(client, view, revision, resources_ready, true)
    }
    fn acknowledge_applied_inner(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        revision: u64,
        resources_ready: bool,
        retained_proof: bool,
    ) -> Result<(), SessionError> {
        let conn = self.connection_mut(client, view)?;
        if revision == 0
            || conn.revision.is_none_or(|r| revision > r)
            || conn.acknowledged_revision.is_some_and(|r| revision < r)
            || !(retained_proof
                || conn.snapshot_revision == Some(revision)
                || conn.acknowledged_revision == Some(revision)
                || conn.history.iter().any(|h| h.revision == revision))
        {
            return Err(SessionError::WrongRevision);
        }
        conn.acknowledged_revision = Some(revision);
        conn.resources_ready = resources_ready;
        if !resources_ready {
            conn.presented_revision = None;
            conn.semantic_ready_revision = None;
        }
        Ok(())
    }
    pub fn mark_resources_missing(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<(), SessionError> {
        let conn = self.connection_mut(client, view)?;
        conn.resources_ready = false;
        conn.presented_revision = None;
        conn.semantic_ready_revision = None;
        Ok(())
    }
    pub fn acknowledge_presented(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        revision: u64,
    ) -> Result<(), SessionError> {
        self.authorize_presented(client, view, revision)?;
        let conn = self.connection_mut(client, view)?;
        if conn.presented_revision.is_some_and(|old| revision < old) {
            return Err(SessionError::WrongRevision);
        }
        conn.presented_revision = Some(revision);
        Ok(())
    }
    pub fn authorize_presented(
        &self,
        client: &AuthenticatedClient,
        view: Id,
        revision: u64,
    ) -> Result<(), SessionError> {
        let conn = self.connection(client, view)?;
        if conn.semantic_only
            || revision == 0
            || !conn.resources_ready
            || conn.acknowledged_revision.is_none_or(|r| revision > r)
            || !(conn.snapshot_revision == Some(revision)
                || conn.acknowledged_revision == Some(revision)
                || conn.history.iter().any(|entry| entry.revision == revision))
        {
            return Err(SessionError::NotPresentable);
        }
        Ok(())
    }

    /// Explicit validated semantic consumption, never visual presentation. Only
    /// semantic-family commands may use this readiness, and only in a negotiated
    /// semantics.only.v1 view. Resource and reconnect resets clear the marker.
    pub fn acknowledge_semantic_ready(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        revision: u64,
    ) -> Result<(), SessionError> {
        let conn = self.connection_mut(client, view)?;
        if !conn.semantic_only
            || revision == 0
            || !conn.resources_ready
            || conn.acknowledged_revision != Some(revision)
        {
            return Err(SessionError::NotPresentable);
        }
        if conn
            .semantic_ready_revision
            .is_some_and(|old| revision < old)
        {
            return Err(SessionError::WrongRevision);
        }
        conn.semantic_ready_revision = Some(revision);
        Ok(())
    }
    fn prior_request(
        &self,
        client: &AuthenticatedClient,
        request: Id,
        payload: &RequestPayload,
    ) -> Result<Option<RequestResult>, SessionError> {
        match self.requests.get(&(client.clone(), request)) {
            Some(record) if record.payload != *payload => Err(SessionError::ChangedRequest),
            Some(record) => Ok(Some(record.result.clone())),
            None => Ok(None),
        }
    }
    fn reserve_request(
        &mut self,
        client: &AuthenticatedClient,
        request: Id,
        payload: RequestPayload,
        result: RequestResult,
    ) -> Result<(), SessionError> {
        if self.requests.len() >= self.config.max_control_requests {
            return Err(SessionError::RetentionExhausted);
        }
        self.requests
            .insert((client.clone(), request), RequestRecord { payload, result });
        Ok(())
    }
    pub fn acquire_control(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        request: Id,
        now: u64,
    ) -> Result<ControlLease, SessionError> {
        self.tick(now);
        let conn = self.connection(client, view)?;
        if !conn.input_ready(true) {
            return Err(SessionError::Unauthorized);
        }
        let mut allowed_actions = conn.authorization.permissions.actions.clone();
        if conn.semantic_only {
            allowed_actions.retain(|a| {
                !matches!(
                    a.as_str(),
                    "RawInputBatch" | "TextComposition" | "ResizeRequest"
                )
            });
        }
        let payload = RequestPayload::Acquire { view };
        if let Some(RequestResult::Grant(grant)) = self.prior_request(client, request, &payload)? {
            return self
                .active
                .as_ref()
                .filter(|a| a.lease.client == *client && a.lease.generation == grant.generation)
                .map(|a| a.lease.clone())
                .ok_or(SessionError::StaleRequest);
        }
        if self.requests.len() >= self.config.max_control_requests {
            return Err(SessionError::RetentionExhausted);
        }
        if !self.cleanup.is_empty() {
            return Err(SessionError::ControlBusy);
        }
        if let Some(active) = &self.active {
            if active.lease.client != *client || active.lease.view_id != view {
                return Err(SessionError::ControlBusy);
            }
            let lease = active.lease.clone();
            self.reserve_request(
                client,
                request,
                payload,
                RequestResult::Grant(lease.clone()),
            )?;
            return Ok(lease);
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(SessionError::ClockExhausted)?;
        let expires_at_ms = self
            .last_now
            .checked_add(self.config.lease_duration_ms)
            .ok_or(SessionError::ClockExhausted)?;
        let lease = ControlLease {
            lease_id: Self::identifier(self.epoch, generation),
            generation,
            client: client.clone(),
            view_id: view,
            expires_at_ms,
            allowed_actions,
        };
        self.reserve_request(
            client,
            request,
            payload,
            RequestResult::Grant(lease.clone()),
        )?;
        self.generation = generation;
        self.active = Some(ActiveControl {
            lease: lease.clone(),
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            touches: BTreeSet::new(),
            composition: None,
        });
        Ok(lease)
    }
    pub fn renew_control(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        request: Id,
        lease: Id,
        generation: u64,
        now: u64,
    ) -> Result<ControlLease, SessionError> {
        self.tick(now);
        self.check_control_scope(client, view, lease, generation, true)?;
        let payload = RequestPayload::Renew { lease, generation };
        if let Some(RequestResult::Grant(grant)) = self.prior_request(client, request, &payload)? {
            return Ok(grant);
        }
        if self.requests.len() >= self.config.max_control_requests {
            return Err(SessionError::RetentionExhausted);
        }
        let expiry = self
            .last_now
            .checked_add(self.config.lease_duration_ms)
            .ok_or(SessionError::ClockExhausted)?;
        let mut grant = self.active.as_ref().unwrap().lease.clone();
        grant.expires_at_ms = expiry;
        self.reserve_request(
            client,
            request,
            payload,
            RequestResult::Grant(grant.clone()),
        )?;
        self.active.as_mut().unwrap().lease = grant.clone();
        Ok(grant)
    }
    pub fn release_control(
        &mut self,
        client: &AuthenticatedClient,
        request: Id,
        lease: Id,
        generation: u64,
        now: u64,
    ) -> Result<(), SessionError> {
        self.tick(now);
        let conn = self
            .connections
            .get(&client.client_instance_id)
            .ok_or(SessionError::Unauthenticated)?;
        if conn.identity != *client {
            return Err(SessionError::WrongPrincipal);
        }
        let payload = RequestPayload::Release { lease, generation };
        if matches!(
            self.prior_request(client, request, &payload)?,
            Some(RequestResult::Released)
        ) {
            return Ok(());
        }
        if generation == 0
            || generation > self.generation
            || lease != Self::identifier(self.epoch, generation)
        {
            return Err(SessionError::StaleControl);
        }
        if let Some(active) = &self.active {
            if active.lease.generation == generation {
                if active.lease.client != *client || active.lease.lease_id != lease {
                    return Err(SessionError::StaleControl);
                }
                self.revoke_active(CleanupReason::Released);
            }
        }
        // Cleanup must remain available even after the request-retention cap fills.
        // An old-generation release is intrinsically idempotent and cannot affect
        // a subsequent lease, so remembering this reply is optional at exhaustion.
        if self.requests.len() < self.config.max_control_requests {
            self.reserve_request(client, request, payload, RequestResult::Released)?;
        }
        Ok(())
    }
    fn check_control(
        &self,
        client: &AuthenticatedClient,
        view: Id,
        lease: Id,
        generation: u64,
    ) -> Result<(), SessionError> {
        self.check_control_scope(client, view, lease, generation, false)
    }
    fn check_control_scope(
        &self,
        client: &AuthenticatedClient,
        view: Id,
        lease: Id,
        generation: u64,
        allow_semantic: bool,
    ) -> Result<(), SessionError> {
        let conn = self.connection(client, view)?;
        if !conn.input_ready(allow_semantic) {
            return Err(SessionError::Unauthorized);
        }
        let active = self.active.as_ref().ok_or(SessionError::StaleControl)?;
        if active.lease.client != *client
            || active.lease.view_id != view
            || active.lease.lease_id != lease
            || active.lease.generation != generation
            || active.lease.expires_at_ms <= self.last_now
        {
            return Err(SessionError::StaleControl);
        }
        Ok(())
    }
    pub fn validate_control(
        &mut self,
        client: &AuthenticatedClient,
        command: &SemanticCommand,
        now: u64,
    ) -> Result<(), Rejection> {
        self.validate_family_control(
            client,
            &command.header(),
            CommandFamily::Semantic,
            &command.action_id,
            now,
        )
    }
    /// Trusted ledger admission calls this with the header/permission decoded
    /// from the retained exact wire payload. This does not validate source
    /// topology, editing ownership, target clocks or raw routing; the cooperating
    /// host must validate those inside the same dispatch boundary.
    pub fn validate_command_control(
        &mut self,
        client: &AuthenticatedClient,
        header: &CommandHeader,
        operation: &str,
        now: u64,
    ) -> Result<(), Rejection> {
        self.validate_family_control(client, header, CommandFamily::RawInput, operation, now)
    }
    pub fn validate_family_control(
        &mut self,
        client: &AuthenticatedClient,
        header: &CommandHeader,
        family: CommandFamily,
        operation: &str,
        now: u64,
    ) -> Result<(), Rejection> {
        self.tick(now);
        if header.epoch != self.epoch || header.client_instance_id != client.client_instance_id {
            return Err(Rejection::Unauthorized);
        }
        self.check_control_scope(
            client,
            header.view_id,
            header.control_lease_id,
            header.control_generation,
            family == CommandFamily::Semantic,
        )
        .map_err(|error| match error {
            SessionError::WrongView => Rejection::WrongView,
            SessionError::StaleControl => Rejection::StaleControl,
            _ => Rejection::Unauthorized,
        })?;
        if !self
            .active
            .as_ref()
            .unwrap()
            .lease
            .allowed_actions
            .contains(operation)
        {
            return Err(Rejection::Unauthorized);
        }
        Ok(())
    }
    fn held_capacity(&self) -> Result<(), SessionError> {
        let active = self.active.as_ref().ok_or(SessionError::StaleControl)?;
        if active.keys.len() + active.buttons.len() + active.touches.len()
            >= self.config.max_held_inputs
        {
            return Err(SessionError::RetentionExhausted);
        }
        Ok(())
    }
    pub fn record_key(
        &mut self,
        lease: &ControlLease,
        key: PhysicalKey,
        pressed: bool,
        now: u64,
    ) -> Result<(), SessionError> {
        self.tick(now);
        self.check_control(
            &lease.client,
            lease.view_id,
            lease.lease_id,
            lease.generation,
        )?;
        if key.namespace.is_empty() || key.namespace.len() > 128 {
            return Err(SessionError::Oversized);
        }
        if pressed && !self.active.as_ref().unwrap().keys.contains(&key) {
            self.held_capacity()?;
        }
        let active = self.active.as_mut().unwrap();
        if pressed {
            active.keys.insert(key);
        } else {
            active.keys.remove(&key);
        }
        Ok(())
    }
    pub fn record_button(
        &mut self,
        lease: &ControlLease,
        button: u32,
        pressed: bool,
        now: u64,
    ) -> Result<(), SessionError> {
        self.tick(now);
        self.check_control(
            &lease.client,
            lease.view_id,
            lease.lease_id,
            lease.generation,
        )?;
        if pressed && !self.active.as_ref().unwrap().buttons.contains(&button) {
            self.held_capacity()?;
        }
        let active = self.active.as_mut().unwrap();
        if pressed {
            active.buttons.insert(button);
        } else {
            active.buttons.remove(&button);
        }
        Ok(())
    }
    pub fn record_touch(
        &mut self,
        lease: &ControlLease,
        touch: u64,
        active_touch: bool,
        now: u64,
    ) -> Result<(), SessionError> {
        self.tick(now);
        self.check_control(
            &lease.client,
            lease.view_id,
            lease.lease_id,
            lease.generation,
        )?;
        if active_touch && !self.active.as_ref().unwrap().touches.contains(&touch) {
            self.held_capacity()?;
        }
        let active = self.active.as_mut().unwrap();
        if active_touch {
            active.touches.insert(touch);
        } else {
            active.touches.remove(&touch);
        }
        Ok(())
    }
    pub fn set_composition(
        &mut self,
        lease: &ControlLease,
        composition: Option<Id>,
        now: u64,
    ) -> Result<(), SessionError> {
        self.tick(now);
        self.check_control(
            &lease.client,
            lease.view_id,
            lease.lease_id,
            lease.generation,
        )?;
        self.active.as_mut().unwrap().composition = composition;
        Ok(())
    }
    pub fn current_lease(&mut self, now: u64) -> Option<ControlLease> {
        self.tick(now);
        self.active.as_ref().map(|a| a.lease.clone())
    }
}
