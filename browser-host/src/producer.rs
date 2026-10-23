use crate::projection::{self, Capture};
use prost::Message;
use rui_producer_sdk::publication_builder;
use rui_protocol_core::{
    commands::{
        Arguments, Command, CommandFamily, CommandLedger, DispatchFailure, DispatchOutcome, Id,
        LedgerLimits, Rejection, SemanticAuthority, TargetState, ValueSchema,
    },
    resources::MemoryBudget,
    session::{SessionBroker, SessionConfig, SharedPayload},
    wire::proto as p,
};
use rui_transport::{producer::*, TransportError};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    os::unix::net::UnixStream,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

pub const PROFILE: &str = "browser.semantic.v1";
pub fn fresh_id() -> Id {
    let mut id = [0; 16];
    getrandom::getrandom(&mut id).expect("operating system entropy");
    id
}
pub struct Worker {
    socket: UnixStream,
}
impl Worker {
    pub fn connect(path: &std::path::Path) -> std::io::Result<Self> {
        let socket = UnixStream::connect(path)?;
        socket.set_read_timeout(Some(Duration::from_secs(5)))?;
        socket.set_write_timeout(Some(Duration::from_secs(2)))?;
        Ok(Self { socket })
    }
    pub fn call(&mut self, request: Value) -> Result<Value, TransportError> {
        let bytes = serde_json::to_vec(&request).map_err(|_| TransportError::Protocol)?;
        if bytes.len() > 65536 {
            return Err(TransportError::Limit);
        }
        self.socket
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .map_err(|_| TransportError::Io)?;
        self.socket
            .write_all(&bytes)
            .map_err(|_| TransportError::Io)?;
        let mut size = [0; 4];
        self.socket
            .read_exact(&mut size)
            .map_err(|_| TransportError::Deadline)?;
        let size = u32::from_be_bytes(size) as usize;
        if size > 1024 * 1024 {
            return Err(TransportError::Limit);
        }
        let mut bytes = vec![0; size];
        self.socket
            .read_exact(&mut bytes)
            .map_err(|_| TransportError::Io)?;
        serde_json::from_slice(&bytes).map_err(|_| TransportError::Protocol)
    }
}
struct Observation {
    capture: Arc<Capture>,
    snapshot: p::Snapshot,
}
impl Observation {
    fn read(worker: &mut Worker) -> Result<Self, TransportError> {
        let capture = serde_json::from_value(worker.call(json!({"op":"capture"}))?)
            .map_err(|_| TransportError::Protocol)?;
        let snapshot = projection::snapshot(&capture).map_err(|_| TransportError::Protocol)?;
        Ok(Self {
            capture: Arc::new(capture),
            snapshot,
        })
    }
}
struct Scope {
    alive: AtomicBool,
    owner: Arc<()>,
    opened: AtomicBool,
}
struct View {
    session: rui_protocol_core::session::NegotiatedSession,
    scope: Arc<Scope>,
    lifetime: AuthorizationLifetime,
    snapshot: p::Snapshot,
    capture: Arc<Capture>,
    pending: Option<SharedPayload>,
    ready: bool,
    revoked: Option<Revoked>,
}
pub struct State {
    sessions: SessionBroker,
    ledger: CommandLedger,
    epoch: Id,
    session: Id,
    views: BTreeMap<Id, View>,
    worker: Worker,
    observation: Observation,
    last_capture: Instant,
    origin: Instant,
    stopped: Arc<AtomicBool>,
    identity: Arc<()>,
    selection: String,
}
#[derive(Clone)]
pub struct BrowserPort {
    state: Arc<Mutex<State>>,
    budget: MemoryBudget,
    admission: Arc<Semaphore>,
    stopped: Arc<AtomicBool>,
    identity: Arc<()>,
}
impl BrowserPort {
    pub fn invite(&self) -> Result<(), TransportError> {
        self.state
            .lock()
            .map_err(|_| TransportError::Authority)?
            .worker
            .call(json!({"op":"invite"}))?;
        Ok(())
    }
    pub fn desktop_command(&self, op: &str, id: Option<&str>) -> Result<Value, TransportError> {
        let mut state = self.state.lock().map_err(|_| TransportError::Authority)?;
        if state.selection != "desktop" {
            return Err(TransportError::Protocol);
        }
        let result = state.worker.call(json!({"op":op,"id":id}))?;
        state.last_capture = Instant::now() - Duration::from_secs(1);
        Ok(result)
    }
    pub fn new(mut worker: Worker) -> Result<Self, TransportError> {
        let observation = Observation::read(&mut worker)?;
        let session = fresh_id();
        let epoch = fresh_id();
        let budget = MemoryBudget::new(128 << 20);
        let mut config = SessionConfig {
            max_clients: 32,
            max_history_records: 16,
            max_history_bytes: 2 << 20,
            ..Default::default()
        };
        for profile in ["semantics.only.v1", PROFILE] {
            config.supported_profiles.insert(profile.into());
            config.required_profiles.insert(profile.into());
        }
        let sessions = SessionBroker::with_memory_budget(session, epoch, config, budget.clone())
            .map_err(|_| TransportError::Limit)?;
        let identity = Arc::new(());
        let stopped = Arc::new(AtomicBool::new(false));
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                sessions,
                ledger: CommandLedger::new(epoch, LedgerLimits::default())
                    .map_err(|_| TransportError::Limit)?,
                epoch,
                session,
                views: BTreeMap::new(),
                worker,
                observation,
                last_capture: Instant::now(),
                origin: Instant::now(),
                stopped: stopped.clone(),
                identity: identity.clone(),
                selection: if std::env::var("RUI_WORKSPACE").as_deref() == Ok("desktop") {
                    "desktop".into()
                } else {
                    "browser".into()
                },
            })),
            budget,
            admission: Arc::new(Semaphore::new(16)),
            stopped,
            identity,
        })
    }
    pub fn secret_scope(&self, client: Id, target: u64) -> Result<Value, TransportError> {
        let mut s = self.state.lock().map_err(|_| TransportError::Authority)?;
        let now = s.now();
        s.drain(now);
        let lease = s
            .sessions
            .current_lease(now)
            .ok_or(TransportError::Authority)?;
        if lease.client.client_instance_id != client {
            return Err(TransportError::Authority);
        }
        let v = s.views.get(&client).ok_or(TransportError::Authority)?;
        if !v.scope.alive.load(Ordering::Acquire) || !v.lifetime.is_authorized() {
            return Err(TransportError::Authority);
        }
        let n = v
            .capture
            .nodes
            .iter()
            .find(|n| n.id == target && n.kind.as_deref() == Some("password"))
            .ok_or(TransportError::Authority)?;
        Ok(
            json!({"client":hex(&client),"target":target,"version":n.version,"lease":hex(&lease.lease_id),"generation":lease.generation,"epoch":hex(&s.epoch)}),
        )
    }
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn unhex<const N: usize>(v: &str) -> Option<[u8; N]> {
    if v.len() != N * 2 || !v.is_ascii() {
        return None;
    }
    let mut out = [0; N];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&v[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}
fn authority(v: &View) -> SemanticAuthority {
    let mut targets = BTreeMap::new();
    for record in &v.snapshot.records {
        if let Some(p::record::Record::Semantic(n)) = &record.record {
            targets.insert(
                n.id,
                TargetState {
                    enabled: !n.states.iter().any(|s| {
                        s.key == p::StateKey::Disabled as i32 && s.value == p::TriState::True as i32
                    }),
                    target_version: n.version,
                    text_version: None,
                    model_version: None,
                    focus_generation: None,
                    text_scalar_count: None,
                    value_schema: n.typed_value.as_ref().and_then(|v| match &v.value {
                        Some(p::typed_value::Value::Boolean(_)) => Some(ValueSchema::Boolean),
                        Some(p::typed_value::Value::Text(_)) => {
                            Some(ValueSchema::Text { max_scalars: 16384 })
                        }
                        _ => None,
                    }),
                    actions: n
                        .actions
                        .iter()
                        .map(|a| (a.id.clone(), a.schema_version))
                        .collect(),
                },
            );
        }
    }
    SemanticAuthority {
        view_id: v.session.view_id,
        current_view_revision: v.snapshot.revision,
        allowed_actions: v.session.permissions.actions.clone(),
        targets,
    }
}
impl State {
    fn now(&self) -> u64 {
        self.origin.elapsed().as_millis() as u64
    }
    fn drain(&mut self, now: u64) {
        self.sessions.tick(now);
        for v in self.views.values_mut() {
            if !v.scope.alive.load(Ordering::Acquire)
                || !v.lifetime.is_authorized()
                || self.stopped.load(Ordering::Acquire)
            {
                v.scope.alive.store(false, Ordering::Release);
                let _ = self.sessions.disconnect(&v.session.client, now);
                if !v.lifetime.is_authorized() {
                    let _ = self
                        .sessions
                        .invalidate_authorization(&v.session.client, now);
                }
            }
        }
        while let Some(cleanup) = self.sessions.take_cleanup() {
            if let Some(v) = self.views.get_mut(&cleanup.lease.client.client_instance_id) {
                v.revoked = Some(Revoked {
                    lease: cleanup.lease.lease_id,
                    generation: cleanup.lease.generation,
                    reason: format!("{:?}", cleanup.reason),
                });
            }
        }
    }
    fn refresh(&mut self) -> Result<(), TransportError> {
        if self.last_capture.elapsed() < Duration::from_millis(200) {
            return Ok(());
        }
        self.observation = Observation::read(&mut self.worker)?;
        self.last_capture = Instant::now();
        Ok(())
    }
    fn publish(&mut self, id: Id, now: u64) -> Result<(), TransportError> {
        let v = self.views.get_mut(&id).ok_or(TransportError::Authority)?;
        if !v.ready || Arc::ptr_eq(&v.capture, &self.observation.capture) {
            return Ok(());
        }
        if v.capture == self.observation.capture {
            v.capture = self.observation.capture.clone();
            return Ok(());
        }
        let target = self.observation.snapshot.clone();
        let candidate = publication_builder::build(&v.snapshot, target, &BTreeSet::new())
            .map_err(|_| TransportError::Receiver)?;
        if candidate.transaction.mutations.is_empty() {
            v.capture = self.observation.capture.clone();
            return Ok(());
        }
        let envelope = p::Envelope {
            protocol_version: 1,
            session_id: self.session.to_vec(),
            epoch: self.epoch.to_vec(),
            view_id: v.session.view_id.to_vec(),
            kind: p::MessageKind::Transaction as i32,
            payload: candidate.transaction.encode_to_vec(),
            required_features: vec![],
        };
        self.sessions
            .publish(
                &v.session.client,
                v.session.view_id,
                v.snapshot.revision,
                candidate.snapshot.revision,
                envelope.encode_to_vec(),
                now,
            )
            .map_err(|_| TransportError::Limit)?;
        v.pending = self
            .sessions
            .take_outbound(&v.session.client, v.session.view_id)
            .map_err(|_| TransportError::Authority)?;
        v.snapshot = candidate.snapshot;
        v.capture = self.observation.capture.clone();
        v.ready = false;
        Ok(())
    }
    fn execute(&mut self, request: Request) -> Result<Reply, TransportError> {
        let now = self.now();
        self.drain(now);
        if self.stopped.load(Ordering::Acquire) {
            return Err(TransportError::Closed);
        }
        match request {
            Request::Connect {
                authorization,
                mut hello,
                lifetime,
                ..
            } => {
                if !lifetime.is_authorized() {
                    return Err(TransportError::Authentication);
                }
                let id = hello.client_instance_id;
                if self
                    .views
                    .get(&id)
                    .is_some_and(|v| v.scope.alive.load(Ordering::Acquire))
                {
                    return Err(TransportError::ProducerBusy);
                }
                // Always checkpoint reconnects; the ledger and epoch still survive.
                if let Some(old) = self.views.remove(&id) {
                    self.sessions
                        .invalidate_authorization(&old.session.client, now)
                        .map_err(|_| TransportError::Authority)?;
                }
                hello.resume = None;
                let session = self
                    .sessions
                    .negotiate(Some(authorization), *hello, now)
                    .map_err(|_| TransportError::Authority)?;
                let snapshot = self.observation.snapshot.clone();
                let candidate =
                    publication_builder::build(&p::Snapshot::default(), snapshot, &BTreeSet::new())
                        .map_err(|_| TransportError::Receiver)?;
                let snapshot = candidate.snapshot;
                let envelope = p::Envelope {
                    protocol_version: 1,
                    session_id: self.session.to_vec(),
                    epoch: self.epoch.to_vec(),
                    view_id: session.view_id.to_vec(),
                    kind: p::MessageKind::Snapshot as i32,
                    payload: snapshot.encode_to_vec(),
                    required_features: vec![],
                };
                let bytes = envelope.encode_to_vec();
                self.sessions
                    .install_snapshot(
                        &session.client,
                        session.view_id,
                        snapshot.revision,
                        bytes.len(),
                    )
                    .map_err(|_| TransportError::Limit)?;
                let pending = self
                    .sessions
                    .snapshot_publication_payload(&session.client, session.view_id, bytes, None)
                    .map_err(|_| TransportError::Limit)?;
                let memory = self
                    .sessions
                    .reserve_view_memory(&session.client, session.view_id, 8 << 20)
                    .map_err(|_| TransportError::Limit)?;
                let scope = Arc::new(Scope {
                    alive: AtomicBool::new(true),
                    opened: AtomicBool::new(false),
                    owner: self.identity.clone(),
                });
                let connection = Connection::issued(
                    session.clone(),
                    None,
                    lifetime.clone(),
                    Box::new(scope.clone()),
                    memory,
                );
                self.views.insert(
                    id,
                    View {
                        session,
                        scope,
                        lifetime,
                        snapshot,
                        capture: self.observation.capture.clone(),
                        pending: Some(pending),
                        ready: false,
                        revoked: None,
                    },
                );
                Ok(Reply::Connected(connection))
            }
            Request::Scoped {
                connection,
                operation,
                ..
            } => {
                let scope = connection
                    .capability::<Arc<Scope>>()
                    .ok_or(TransportError::Authority)?
                    .clone();
                if !Arc::ptr_eq(&scope.owner, &self.identity)
                    || !scope.alive.load(Ordering::Acquire)
                    || !connection.authorized()
                {
                    return Err(TransportError::Authentication);
                }
                let client = connection.session().client.clone();
                let id = client.client_instance_id;
                let view = connection.session().view_id;
                self.sessions
                    .authorize_projection(&client, view)
                    .map_err(|_| TransportError::Authority)?;
                if !scope.opened.load(Ordering::Acquire)
                    && !matches!(
                        operation,
                        Operation::Open { .. }
                            | Operation::Authorize
                            | Operation::ReserveMemory { .. }
                    )
                {
                    return Err(TransportError::Protocol);
                }
                match operation {
                    Operation::Authorize => Ok(Reply::Done),
                    Operation::Open {
                        selection,
                        mirror_canvas_policy,
                    } => {
                        if selection != self.selection
                            || mirror_canvas_policy != "semantic-only"
                            || scope.opened.swap(true, Ordering::AcqRel)
                        {
                            return Err(TransportError::Protocol);
                        }
                        Ok(Reply::Opened {
                            description: ViewDescription {
                                next_command_sequence: self
                                    .ledger
                                    .high_water(&client)
                                    .checked_add(1),
                                width: 1280.,
                                height: 900.,
                                coverage: "semantic-only".into(),
                                input_modes: vec!["semantic.v1".into()],
                                input_devices: vec![],
                            },
                            checkpoint: self.views.get_mut(&id).unwrap().pending.take(),
                        })
                    }
                    Operation::Ack(ack) => {
                        match ack {
                            Acknowledgement::Applied { revision, witness } => {
                                if witness.publication_revision().is_some() {
                                    self.sessions.acknowledge_applied_publication(
                                        &client, view, revision, true, &witness,
                                    )
                                } else {
                                    self.sessions
                                        .acknowledge_applied(&client, view, revision, true)
                                }
                                .map_err(|_| TransportError::Authority)?;
                            }
                            Acknowledgement::SemanticReady { revision } => {
                                self.sessions
                                    .acknowledge_semantic_ready(&client, view, revision)
                                    .map_err(|_| TransportError::Authority)?;
                                if self.views[&id].snapshot.revision == revision {
                                    self.views.get_mut(&id).unwrap().ready = true;
                                }
                            }
                            _ => return Err(TransportError::Protocol),
                        };
                        Ok(Reply::Done)
                    }
                    Operation::Control(c) => match c {
                        Control::Acquire { request } => self
                            .sessions
                            .acquire_control(&client, view, request, now)
                            .map(Reply::Lease)
                            .map_err(|_| TransportError::Authority),
                        Control::Renew {
                            request,
                            lease,
                            generation,
                        } => self
                            .sessions
                            .renew_control(&client, view, request, lease, generation, now)
                            .map(Reply::Lease)
                            .map_err(|_| TransportError::Authority),
                        Control::Release {
                            request,
                            lease,
                            generation,
                        } => {
                            self.sessions
                                .release_control(&client, request, lease, generation, now)
                                .map_err(|_| TransportError::Authority)?;
                            Ok(Reply::Done)
                        }
                    },
                    Operation::Command { family, bytes } => {
                        let auth = authority(&self.views[&id]);
                        let capture = self.views[&id].capture.clone();
                        let worker = &mut self.worker;
                        let stopped = self.stopped.clone();
                        let started = Instant::now();
                        let lease = self.sessions.current_lease(now);
                        let result = self
                            .ledger
                            .submit_controlled(
                                &mut self.sessions,
                                &client,
                                family,
                                &bytes,
                                now,
                                |command| {
                                    if family != CommandFamily::Semantic {
                                        return Err(Rejection::UnsupportedAction);
                                    }
                                    let Command::Semantic(command) = command else {
                                        return Err(Rejection::UnsupportedAction);
                                    };
                                    auth.validate(command)
                                },
                                |command| {
                                    let Command::Semantic(command) = command else {
                                        return Err(DispatchFailure::Failed);
                                    };
                                    if stopped.load(Ordering::Acquire)
                                        || !scope.alive.load(Ordering::Acquire)
                                        || !connection.authorized()
                                    {
                                        return Err(DispatchFailure::Failed);
                                    }
                                    let remaining = lease.as_ref().map_or(0, |lease| {
                                        lease.expires_at_ms.saturating_sub(
                                            now + started.elapsed().as_millis() as u64,
                                        )
                                    });
                                    if remaining == 0 {
                                        return Err(DispatchFailure::Failed);
                                    }
                                    let Some(node) = capture
                                        .nodes
                                        .iter()
                                        .find(|node| node.id == command.target_id)
                                    else {
                                        return Err(DispatchFailure::Failed);
                                    };
                                    let value = match &command.arguments {
                                        Arguments::ValueText(value) => json!(value),
                                        Arguments::Checked(value) => json!(value),
                                        Arguments::None => Value::Null,
                                        _ => return Err(DispatchFailure::Failed),
                                    };
                                    let reply = worker.call(json!({
                                    "op": "dispatch", "target": command.target_id,
                                    "version": node.version, "action": command.action_id,
                                    "value": value, "client": hex(&client.client_instance_id),
                                    "lease": hex(&command.control_lease_id),
                                    "generation": command.control_generation,
                                    "epoch": hex(&command.epoch), "deadlineMs": remaining.min(2000),
                                })).map_err(|_| DispatchFailure::OutcomeUnknown)?;
                                    match reply["status"].as_str() {
                                        Some("dispatched") => Ok(DispatchOutcome::Dispatched),
                                        Some("rejected") => Err(DispatchFailure::Failed),
                                        _ => Err(DispatchFailure::OutcomeUnknown),
                                    }
                                },
                            )
                            .map_err(|_| TransportError::Protocol)?;
                        self.last_capture = Instant::now() - Duration::from_secs(1);
                        Ok(Reply::Command(result))
                    }
                    Operation::Result { sequence } => self
                        .ledger
                        .get_result(&client, self.epoch, sequence, now)
                        .map(Reply::Command)
                        .map_err(|_| TransportError::Protocol),
                    Operation::Pump { take_publication } => {
                        self.refresh()?;
                        self.publish(id, now)?;
                        let v = self.views.get_mut(&id).unwrap();
                        Ok(Reply::Pump {
                            publication: if take_publication {
                                v.pending.take()
                            } else {
                                None
                            },
                            revoked: v.revoked.take(),
                        })
                    }
                    Operation::ReserveMemory { bytes } => self
                        .sessions
                        .reserve_view_memory(&client, view, bytes)
                        .map(Reply::Memory)
                        .map_err(|_| TransportError::Limit),
                    Operation::Resources { digests, .. } if digests.is_empty() => {
                        Ok(Reply::Resources(vec![]))
                    }
                    Operation::Evicted {
                        digests,
                        previously_missing,
                        ..
                    } if digests.is_empty() && previously_missing.is_empty() => {
                        Ok(Reply::Evicted { missing: vec![] })
                    }
                    _ => Err(TransportError::Protocol),
                }
            }
        }
    }
}
impl ProducerPort for BrowserPort {
    fn transact(&self, request: Request) -> PortFuture<'_> {
        let permit = self.admission.clone().try_acquire_owned();
        let state = self.state.clone();
        let charge = self.budget.reserve(2 << 20);
        Box::pin(async move {
            let _permit = permit.map_err(|_| TransportError::Limit)?;
            let charge = charge.map_err(|_| TransportError::Limit)?;
            tokio::task::spawn_blocking(move || {
                let value = state
                    .lock()
                    .map_err(|_| TransportError::Authority)?
                    .execute(request)?;
                Ok(Received::charged(value, charge, None))
            })
            .await
            .map_err(|_| TransportError::Authority)?
        })
    }
    fn disconnect(&self, c: &Connection) {
        if let Some(scope) = c
            .capability::<Arc<Scope>>()
            .filter(|s| Arc::ptr_eq(&s.owner, &self.identity))
        {
            scope.alive.store(false, Ordering::Release);
        }
    }
    fn shutdown(&self) {
        self.stopped.store(true, Ordering::Release);
    }
    fn flush_lifecycle(&self, _now: u64) -> LifecycleFuture<'_> {
        let state = self.state.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let mut s = state.lock().map_err(|_| TransportError::Authority)?;
                let now = s.now();
                s.drain(now);
                Ok(())
            })
            .await
            .map_err(|_| TransportError::Authority)?
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rui_protocol_core::session::{ClientHello, Permissions, VerifiedAuthorization};
    use std::thread;

    fn capture(value: &str, version: u64) -> Value {
        json!({"nodes": [
            {"id": 3, "version": 1, "role": "root", "label": "Desktop", "children": [10]},
            {"id": 10, "version": version, "role": "text", "label": "Document",
             "kind": "text", "value": value, "actions": ["SetValue"]}
        ], "status": "Available"})
    }

    struct Fixture {
        port: BrowserPort,
        source: Arc<Mutex<Value>>,
        requests: Arc<Mutex<Vec<Value>>>,
        worker: thread::JoinHandle<()>,
    }

    fn fixture() -> Fixture {
        let (socket, mut peer) = UnixStream::pair().unwrap();
        let source = Arc::new(Mutex::new(capture("before", 40)));
        let response = source.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        let worker = thread::spawn(move || {
            let mut size = [0; 4];
            while peer.read_exact(&mut size).is_ok() {
                let mut request = vec![0; u32::from_be_bytes(size) as usize];
                peer.read_exact(&mut request).unwrap();
                let request: Value = serde_json::from_slice(&request).unwrap();
                let reply = match request["op"].as_str() {
                    Some("capture") => response.lock().unwrap().clone(),
                    Some("dispatch") => json!({"status": "rejected"}),
                    _ => panic!("unexpected worker request"),
                };
                log.lock().unwrap().push(request);
                let bytes = serde_json::to_vec(&reply).unwrap();
                peer.write_all(&(bytes.len() as u32).to_be_bytes()).unwrap();
                peer.write_all(&bytes).unwrap();
            }
        });
        Fixture {
            port: BrowserPort::new(Worker { socket }).unwrap(),
            source,
            requests,
            worker,
        }
    }

    fn scoped(state: &mut State, connection: &Connection, operation: Operation) -> Reply {
        state
            .execute(Request::Scoped {
                connection: connection.clone(),
                operation,
                now: 0,
            })
            .unwrap()
    }

    fn connect(state: &mut State, id: u8) -> (Connection, SharedPayload) {
        let mut profiles = SessionConfig::default().supported_profiles;
        profiles.extend(["semantics.only.v1".into(), PROFILE.into()]);
        let Reply::Connected(connection) = state
            .execute(Request::Connect {
                authorization: VerifiedAuthorization {
                    principal: format!("client-{id}"),
                    policy_version: 1,
                    permissions: Permissions {
                        can_view: true,
                        can_control: true,
                        actions: ["SetValue".into()].into_iter().collect(),
                    },
                },
                hello: Box::new(ClientHello {
                    client_instance_id: [id; 16],
                    supported_versions: [1].into_iter().collect(),
                    supported_profiles: profiles.clone(),
                    required_profiles: profiles,
                    receiver_limits: Default::default(),
                    resume: None,
                }),
                lifetime: AuthorizationLifetime::new(),
                fresh_view: false,
                now: 0,
            })
            .unwrap()
        else {
            panic!("expected connection")
        };
        let selection = state.selection.clone();
        let Reply::Opened {
            checkpoint: Some(checkpoint),
            ..
        } = scoped(
            state,
            &connection,
            Operation::Open {
                selection,
                mirror_canvas_policy: "semantic-only".into(),
            },
        )
        else {
            panic!("expected checkpoint")
        };
        (connection, checkpoint)
    }

    fn ready(state: &mut State, connection: &Connection, witness: SharedPayload, revision: u64) {
        for ack in [
            Acknowledgement::Applied { revision, witness },
            Acknowledgement::SemanticReady { revision },
        ] {
            scoped(state, connection, Operation::Ack(ack));
        }
    }

    #[test]
    fn views_share_observations_but_publish_independent_revisions() {
        let Fixture {
            port,
            source,
            requests,
            worker,
        } = fixture();
        {
            let mut state = port.state.lock().unwrap();
            let (first, first_frame) = connect(&mut state, 1);
            let (second, second_frame) = connect(&mut state, 2);
            ready(&mut state, &first, first_frame, 1);
            let original = state.observation.capture.clone();
            for view in state.views.values() {
                assert!(Arc::ptr_eq(&original, &view.capture));
                assert_eq!(authority(view).targets[&10].target_version, 1);
                assert_eq!(view.capture.nodes[1].version, 40);
            }
            *source.lock().unwrap() = capture("after", 41);
            state.last_capture = Instant::now() - Duration::from_secs(1);
            state.refresh().unwrap();
            assert!(!Arc::ptr_eq(&original, &state.observation.capture));
            assert_eq!(original.nodes[1].value, Some(json!("before")));
            let expected = publication_builder::build(
                &state.views[&[1; 16]].snapshot,
                projection::snapshot(&state.observation.capture).unwrap(),
                &BTreeSet::new(),
            )
            .unwrap();
            state.publish([1; 16], 0).unwrap();
            state.publish([2; 16], 0).unwrap();
            let first_view = &state.views[&[1; 16]];
            assert!(Arc::ptr_eq(&first_view.capture, &state.observation.capture));
            assert_eq!(first_view.snapshot, expected.snapshot);
            assert_eq!(first_view.snapshot.revision, 2);
            assert_eq!(first_view.capture.nodes[1].version, 41);
            assert_eq!(authority(first_view).targets[&10].target_version, 2);
            let first_update = first_view.pending.clone().unwrap();
            let envelope = p::Envelope::decode(first_update.as_ref()).unwrap();
            assert_eq!(
                p::Transaction::decode(envelope.payload.as_slice()).unwrap(),
                expected.transaction
            );
            assert!(Arc::ptr_eq(&state.views[&[2; 16]].capture, &original));
            assert_eq!(state.views[&[2; 16]].snapshot.revision, 1);
            ready(&mut state, &second, second_frame, 1);
            let Reply::Lease(lease) = scoped(
                &mut state,
                &second,
                Operation::Control(Control::Acquire { request: [9; 16] }),
            ) else {
                panic!("expected control lease")
            };
            let command = p::SemanticCommand {
                epoch: second.session().epoch.to_vec(),
                view_id: second.session().view_id.to_vec(),
                client_instance_id: second.session().client.client_instance_id.to_vec(),
                command_seq: 1,
                control_lease_id: lease.lease_id.to_vec(),
                control_generation: lease.generation,
                target_id: 10,
                action_id: "SetValue".into(),
                action_schema_version: 1,
                observed_view_revision: 1,
                preconditions: Some(p::Preconditions {
                    target_version: Some(1),
                    ..Default::default()
                }),
                arguments: Some(p::semantic_command::Arguments::SetValue(
                    p::ValueArguments {
                        value: Some(p::value_arguments::Value::Text("edit".into())),
                    },
                )),
            };
            assert!(matches!(
                scoped(
                    &mut state,
                    &second,
                    Operation::Command {
                        family: CommandFamily::Semantic,
                        bytes: command.encode_to_vec().into(),
                    }
                ),
                Reply::Command(_)
            ));
            let dispatched = requests.lock().unwrap().last().unwrap().clone();
            assert_eq!(dispatched["op"], "dispatch");
            assert_eq!(dispatched["version"], 40);
            assert_eq!(dispatched["target"], 10);
            state.publish([2; 16], 0).unwrap();
            assert_eq!(state.views[&[2; 16]].snapshot, expected.snapshot);
            assert!(Arc::ptr_eq(
                &state.views[&[2; 16]].capture,
                &state.observation.capture
            ));
            ready(&mut state, &first, first_update, 2);
            state.views.get_mut(&[1; 16]).unwrap().pending.take();
            state.last_capture = Instant::now() - Duration::from_secs(1);
            state.refresh().unwrap();
            state.publish([1; 16], 0).unwrap();
            assert!(state.views[&[1; 16]].pending.is_none());
            assert_eq!(state.views[&[1; 16]].snapshot, expected.snapshot);
            assert!(Arc::ptr_eq(
                &state.views[&[1; 16]].capture,
                &state.observation.capture
            ));
            source.lock().unwrap()["status"] = json!("Partial observation");
            state.last_capture = Instant::now() - Duration::from_secs(1);
            state.refresh().unwrap();
            let status_only = publication_builder::build(
                &expected.snapshot,
                projection::snapshot(&state.observation.capture).unwrap(),
                &BTreeSet::new(),
            )
            .unwrap();
            state.publish([1; 16], 0).unwrap();
            assert_eq!(state.views[&[1; 16]].snapshot, status_only.snapshot);
            assert!(Arc::ptr_eq(
                &state.views[&[1; 16]].capture,
                &state.observation.capture
            ));
        }
        drop(port);
        worker.join().unwrap();
    }

    #[test]
    fn invalid_observations_leave_current_capture_and_projection_intact() {
        let Fixture {
            port,
            source,
            worker,
            ..
        } = fixture();
        {
            let mut state = port.state.lock().unwrap();
            let original = state.observation.capture.clone();
            let original_projection = state.observation.snapshot.clone();
            for invalid in [json!({"nodes": [], "status": "Unavailable"}), {
                let mut value = capture("invalid", 41);
                value["nodes"][0]["children"] = json!([999]);
                value
            }] {
                *source.lock().unwrap() = invalid;
                let stale = Instant::now() - Duration::from_secs(1);
                state.last_capture = stale;
                assert!(matches!(state.refresh(), Err(TransportError::Protocol)));
                assert!(Arc::ptr_eq(&original, &state.observation.capture));
                assert_eq!(state.observation.snapshot, original_projection);
                assert_eq!(state.last_capture, stale);
            }
        }
        drop(port);
        worker.join().unwrap();
    }
}
