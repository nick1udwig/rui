//! A controlled, headless RUI host fixture and mandatory command dispatch boundary.
//! Credentials are verified outside this crate. Network/TLS, GUI rendering and OS
//! input injection are not implemented here.
mod corpus;
mod fixture;
pub use corpus::{
    CANVAS_ID, CHECKBOX_ID, ITEM_IDS, LIST_ID, MODAL_TOGGLE_ID, POPUP_TOGGLE_ID, PUBLIC_TEXT_ID,
    PUBLIC_TEXT_MODEL_ID, REORDER_ID, SECOND_SURFACE_ID, TOOLTIP_TOGGLE_ID,
};
use fixture::ControlledApplication;
pub use fixture::{
    COUNTER_ID, COUNTER_SCENE_ID, SECRET_ID, SECRET_SCENE_ID, SECRET_TEXT_ID, SURFACE_ID,
};
use prost::Message;
use rui_protocol_core::commands::{
    AuthenticatedClient, Command, CommandFamily, CommandLedger, CommandResult, DispatchFailure,
    DispatchOutcome, Id, LedgerError, LedgerLimits, Rejection, SourceEvidence,
};
use rui_protocol_core::session::{
    ClientHello, ControlLease, InputCleanup, NegotiatedSession, Permissions, ReceiverLimits,
    ResetReason, ResumeDisposition, SessionBroker, SessionConfig, SessionError, SharedPayload,
    VerifiedAuthorization, ViewMemoryReservation,
};
use rui_protocol_core::wire::proto as p;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Explicit fixture content-reading permission in trusted Permissions::actions.
/// Being a controller alone does not authorize reading the private field.
pub const READ_SECRET_PERMISSION: &str = "fixture.read-secret.v1";

#[derive(Clone, Debug)]
pub struct HostConfig {
    pub session_id: Id,
    pub epoch: Id,
    pub session: SessionConfig,
    pub ledger: LedgerLimits,
    pub max_text_bytes: usize,
}
impl Default for HostConfig {
    fn default() -> Self {
        Self {
            session_id: [1; 16],
            epoch: [2; 16],
            session: SessionConfig::default(),
            ledger: LedgerLimits::default(),
            max_text_bytes: 64 * 1024,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationFailure {
    SlowConsumer,
    ReceiverLimit,
    RevisionExhausted,
    ProjectionUnavailable,
}
#[derive(Debug, Error, PartialEq, Eq)]
pub enum HostError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error("fixture text or source clock limit exceeded")]
    FixtureLimit,
    #[error("an explicit coherent checkpoint is required")]
    CheckpointRequired,
    #[error("view closed after publication failure: {0:?}")]
    ViewClosed(PublicationFailure),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewStatus {
    pub revision: Option<u64>,
    pub checkpoint_required: bool,
    pub connected: bool,
    pub closed: Option<PublicationFailure>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ViewDescription {
    pub width: f64,
    pub height: f64,
    pub coverage: String,
    pub input_modes: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostDiagnostics {
    pub source_commit: u64,
    pub counter: i64,
    pub clients: usize,
    pub closed_views: usize,
    pub consumed_commands: usize,
    pub retained_command_bytes: usize,
}
/// Independent cooperating-source capture before transport encoding. Records
/// and source provenance are borrowed; the view budget remains charged until
/// this object drops. Debug deliberately omits all projected content.
pub struct SourceCapture {
    commit: u64,
    records: Vec<p::Record>,
    state: rui_protocol_core::model::State,
    _memory: ViewMemoryReservation,
}
impl SourceCapture {
    pub fn source_commit(&self) -> u64 {
        self.commit
    }
    pub fn records(&self) -> &[p::Record] {
        &self.records
    }
    pub fn state(&self) -> &rui_protocol_core::model::State {
        &self.state
    }
}
impl std::fmt::Debug for SourceCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceCapture")
            .field("source_commit", &self.commit)
            .field("record_count", &self.records.len())
            .finish()
    }
}
struct Projection {
    client: AuthenticatedClient,
    view_id: Id,
    version: u32,
    profiles: BTreeSet<String>,
    limits: ReceiverLimits,
    permissions: Permissions,
    semantic_only: bool,
    revision: Option<u64>,
    capture_commit: u64,
    counter_clock: u64,
    text_clock: u64,
    connected: bool,
    checkpoint_required: bool,
    closed: Option<PublicationFailure>,
    records: BTreeMap<u64, RecordStamp>,
}
#[derive(Clone)]
struct RecordStamp {
    version: u64,
    digest: [u8; 32],
    kind: p::ObjectKind,
}
struct Capture {
    records: Vec<p::Record>,
    stamps: BTreeMap<u64, RecordStamp>,
    mutations: Vec<p::Mutation>,
}
impl Projection {
    fn new(session: &NegotiatedSession) -> Self {
        Self {
            client: session.client.clone(),
            view_id: session.view_id,
            version: session.version,
            profiles: session.profiles.clone(),
            limits: session.limits.clone(),
            permissions: session.permissions.clone(),
            semantic_only: session.profiles.contains("semantics.only.v1"),
            revision: None,
            capture_commit: 0,
            counter_clock: 0,
            text_clock: 0,
            connected: true,
            checkpoint_required: true,
            closed: None,
            records: BTreeMap::new(),
        }
    }
    fn envelope(
        &self,
        session_id: Id,
        epoch: Id,
        kind: p::MessageKind,
        payload: Vec<u8>,
    ) -> Vec<u8> {
        p::Envelope {
            protocol_version: self.version,
            session_id: session_id.to_vec(),
            epoch: epoch.to_vec(),
            view_id: self.view_id.to_vec(),
            kind: kind as i32,
            payload,
            required_features: self.profiles.iter().cloned().collect(),
        }
        .encode_to_vec()
    }
}

/// One host-owned cooperating fixture. Session authority, ledger callbacks and
/// relevant source preconditions are always checked together at this boundary.
/// The source and mutable core components are private to prevent bypassing it.
pub struct ControlledHost {
    session_id: Id,
    epoch: Id,
    sessions: SessionBroker,
    ledger: CommandLedger,
    source: ControlledApplication,
    projections: BTreeMap<Id, Projection>,
}
impl ControlledHost {
    pub fn new(mut config: HostConfig, initial_secret: String) -> Result<Self, HostError> {
        config
            .session
            .supported_profiles
            .insert("semantics.only.v1".into());
        Ok(Self {
            session_id: config.session_id,
            epoch: config.epoch,
            sessions: SessionBroker::new(config.session_id, config.epoch, config.session)?,
            ledger: CommandLedger::new(config.epoch, config.ledger)?,
            source: ControlledApplication::new(initial_secret, config.max_text_bytes)?,
            projections: BTreeMap::new(),
        })
    }
    /// Opt-in cooperating corpus. Public text remains valid independently of
    /// current source glyph coverage; visual gaps are explicitly declared.
    pub fn new_corpus(config: HostConfig, initial_secret: String) -> Result<Self, HostError> {
        let max_text_bytes = config.max_text_bytes;
        let mut host = Self::new(config, initial_secret)?;
        host.source.corpus = Some(corpus::Corpus::new(max_text_bytes));
        Ok(host)
    }
    /// The caller supplies credentials already verified by its trusted auth
    /// adapter. Never deserialize VerifiedAuthorization from client data.
    pub fn negotiate(
        &mut self,
        authorization: VerifiedAuthorization,
        hello: ClientHello,
        now: u64,
    ) -> Result<NegotiatedSession, HostError> {
        let mut negotiated = self.sessions.negotiate(Some(authorization), hello, now)?;
        // This fixture retains complete Envelope bytes rather than independently
        // chunked transaction payloads. A lowered envelope ceiling must also fit
        // every retained resume envelope.
        if matches!(&negotiated.resume, ResumeDisposition::Accepted {transactions,..}
            if transactions.iter().any(|bytes| bytes.len() > negotiated.limits.envelope_bytes))
        {
            negotiated.resume = ResumeDisposition::ResetRequired(ResetReason::HistoryUnavailable);
        }
        match self
            .projections
            .get_mut(&negotiated.client.client_instance_id)
        {
            Some(projection) if projection.view_id == negotiated.view_id => {
                projection.connected = true;
                projection.limits = negotiated.limits.clone();
                if projection.capture_commit != self.source.commit
                    || !matches!(negotiated.resume, ResumeDisposition::Accepted { .. })
                {
                    projection.checkpoint_required = true;
                    if matches!(negotiated.resume, ResumeDisposition::Accepted { .. }) {
                        negotiated.resume =
                            ResumeDisposition::ResetRequired(ResetReason::HistoryUnavailable);
                    }
                }
            }
            _ => {
                self.projections.insert(
                    negotiated.client.client_instance_id,
                    Projection::new(&negotiated),
                );
            }
        }
        Ok(negotiated)
    }
    fn projection(&self, client: &AuthenticatedClient, view: Id) -> Result<&Projection, HostError> {
        let projection = self
            .projections
            .get(&client.client_instance_id)
            .ok_or(SessionError::Unauthenticated)?;
        if projection.client != *client {
            return Err(SessionError::WrongPrincipal.into());
        }
        if projection.view_id != view {
            return Err(SessionError::WrongView.into());
        }
        if let Some(reason) = projection.closed {
            return Err(HostError::ViewClosed(reason));
        }
        self.sessions.authorize_projection(client, view)?;
        Ok(projection)
    }
    /// Install an explicit checkpoint. This intentionally resets queue/history
    /// and acknowledgements; it is the initial/reset path, never a read operation.
    /// Returns one exact bound Snapshot envelope, charged through its last clone.
    pub fn checkpoint(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<SharedPayload, HostError> {
        let projection = self.projection(client, view)?;
        let revision = projection
            .revision
            .map_or(Some(1), |r| r.checked_add(1))
            .ok_or(HostError::FixtureLimit)?;
        let decoded = self.source.decoded_projection_bytes(projection);
        let _working = self.sessions.reserve_view_memory(client, view, decoded)?;
        let capture = capture(&self.source, projection, false)?;
        let snapshot = p::Snapshot {
            revision,
            records: capture.records,
            required_resources: vec![],
            required_features: projection.profiles.iter().cloned().collect(),
        };
        let bytes = projection.envelope(
            self.session_id,
            self.epoch,
            p::MessageKind::Snapshot,
            snapshot.encode_to_vec(),
        );
        let output = self.sessions.snapshot_payload(client, view, bytes)?;
        self.sessions
            .install_snapshot(client, view, revision, decoded)?;
        let projection = self
            .projections
            .get_mut(&client.client_instance_id)
            .unwrap();
        projection.revision = Some(revision);
        projection.capture_commit = self.source.commit;
        projection.counter_clock = self.source.counter_clock;
        projection.text_clock = self.source.text_clock;
        projection.records = capture.stamps;
        projection.checkpoint_required = false;
        Ok(output)
    }
    /// Read the current published capture without clearing history/acks or
    /// changing a revision. Repeated outputs remain memory-accounted.
    pub fn snapshot(
        &self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<SharedPayload, HostError> {
        let projection = self.projection(client, view)?;
        if projection.checkpoint_required || projection.capture_commit != self.source.commit {
            return Err(HostError::CheckpointRequired);
        }
        let revision = projection.revision.ok_or(HostError::CheckpointRequired)?;
        let _working = self.sessions.reserve_view_memory(
            client,
            view,
            self.source.decoded_projection_bytes(projection),
        )?;
        let snapshot = p::Snapshot {
            revision,
            records: capture(&self.source, projection, false)?.records,
            required_resources: vec![],
            required_features: projection.profiles.iter().cloned().collect(),
        };
        let bytes = projection.envelope(
            self.session_id,
            self.epoch,
            p::MessageKind::Snapshot,
            snapshot.encode_to_vec(),
        );
        Ok(self.sessions.snapshot_payload(client, view, bytes)?)
    }
    /// Capture directly from current authoritative application state, independently
    /// of transport bytes or receiver replay. It changes no history or revision.
    pub fn source_capture(
        &self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<SourceCapture, HostError> {
        let projection = self.projection(client, view)?;
        if projection.checkpoint_required || projection.capture_commit != self.source.commit {
            return Err(HostError::CheckpointRequired);
        }
        let memory = self.sessions.reserve_view_memory(
            client,
            view,
            self.source.decoded_projection_bytes(projection) * 3,
        )?;
        let records = capture(&self.source, projection, false)?.records;
        let limits = rui_protocol_core::model::Limits {
            max_records: projection.limits.records,
            max_memory_bytes: projection.limits.view_memory_bytes,
            max_depth: projection.limits.ownership_depth,
            ..Default::default()
        };
        let state = rui_protocol_core::model::State::from_source_records(
            projection.revision.unwrap_or(1),
            records.clone(),
            &limits,
        )
        .map_err(|_| HostError::FixtureLimit)?;
        Ok(SourceCapture {
            commit: self.source.commit,
            records,
            state,
            _memory: memory,
        })
    }
    pub fn canvas_size(&self) -> (f64, f64) {
        self.source
            .corpus
            .as_ref()
            .map_or((160.0, 100.0), |c| (c.width + 280.0, c.height.max(200.0)))
    }
    pub fn view_description(
        &self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<ViewDescription, HostError> {
        let projection = self.projection(client, view)?;
        let (width, height) = self.canvas_size();
        Ok(ViewDescription {
            width,
            height,
            coverage: if projection.semantic_only {
                "semantic-only"
            } else if self.source.corpus.is_some() {
                "partial"
            } else {
                "complete"
            }
            .into(),
            input_modes: if projection.permissions.can_control {
                vec!["semantic.v1".into()]
            } else {
                vec![]
            },
        })
    }
    /// Exactly one ledger admission consumes well-formed rejections. Mandatory
    /// current auth/lease/source validation is inside that admission. Only host
    /// code executes the business operation, followed by independent projections.
    pub fn submit(
        &mut self,
        client: &AuthenticatedClient,
        raw_command: &[u8],
        now: u64,
    ) -> Result<CommandResult, HostError> {
        self.submit_family(client, CommandFamily::Semantic, raw_command, now)
    }
    pub fn submit_family(
        &mut self,
        client: &AuthenticatedClient,
        family: CommandFamily,
        raw_command: &[u8],
        now: u64,
    ) -> Result<CommandResult, HostError> {
        let view = self
            .projections
            .get(&client.client_instance_id)
            .ok_or(SessionError::Unauthenticated)?
            .view_id;
        if raw_command.len() > self.projection(client, view)?.limits.envelope_bytes {
            return Err(SessionError::Oversized.into());
        }
        let source = RefCell::new(&mut self.source);
        let projections = RefCell::new(&mut self.projections);
        let session_id = self.session_id;
        let epoch = self.epoch;
        let result = self.ledger.submit_controlled_with_session(
            &mut self.sessions,
            client,
            family,
            raw_command,
            now,
            |command| {
                let Command::Semantic(command) = command else {
                    return Err(Rejection::UnsupportedAction);
                };
                let all = projections.borrow();
                let projection = all
                    .get(&client.client_instance_id)
                    .ok_or(Rejection::Unauthorized)?;
                if projection.checkpoint_required {
                    return Err(Rejection::WrongView);
                }
                source.borrow().validate(projection, command)
            },
            |command, sessions| {
                let Command::Semantic(command) = command else {
                    return Err(DispatchFailure::Failed);
                };
                let commit = {
                    let all = projections.borrow();
                    let projection = all
                        .get(&client.client_instance_id)
                        .ok_or(DispatchFailure::Failed)?;
                    source
                        .borrow_mut()
                        .apply(projection, command, now)
                        .map_err(|_| DispatchFailure::Failed)?
                };
                publish_projections(
                    sessions,
                    &source.borrow(),
                    &mut projections.borrow_mut(),
                    session_id,
                    epoch,
                    now,
                );
                let revision = projections
                    .borrow()
                    .get(&client.client_instance_id)
                    .filter(|p| p.closed.is_none() && p.capture_commit == commit)
                    .and_then(|p| p.revision);
                let evidence = SourceEvidence::new(format!("fixture-commit-{commit}"), revision)
                    .map_err(|_| DispatchFailure::OutcomeUnknown)?;
                Ok(DispatchOutcome::Applied(evidence))
            },
        )?;
        Ok(result)
    }
    pub fn get_command_result(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        sequence: u64,
        now: u64,
    ) -> Result<CommandResult, HostError> {
        self.projection(client, view)?;
        Ok(self.ledger.get_result(client, self.epoch, sequence, now)?)
    }
    pub fn take_outbound(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<Option<SharedPayload>, HostError> {
        self.projection(client, view)?;
        Ok(self.sessions.take_outbound(client, view)?)
    }
    pub fn acknowledge_applied(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        revision: u64,
    ) -> Result<(), HostError> {
        self.projection(client, view)?;
        self.sessions
            .acknowledge_applied(client, view, revision, true)?;
        Ok(())
    }
    pub fn acknowledge_presented(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        revision: u64,
    ) -> Result<(), HostError> {
        self.projection(client, view)?;
        self.sessions
            .acknowledge_presented(client, view, revision)?;
        Ok(())
    }
    pub fn acknowledge_semantic_ready(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        revision: u64,
    ) -> Result<(), HostError> {
        self.projection(client, view)?;
        self.sessions
            .acknowledge_semantic_ready(client, view, revision)?;
        Ok(())
    }
    pub fn acquire_control(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        request: Id,
        now: u64,
    ) -> Result<ControlLease, HostError> {
        if self.projection(client, view)?.checkpoint_required {
            return Err(HostError::CheckpointRequired);
        }
        Ok(self.sessions.acquire_control(client, view, request, now)?)
    }
    pub fn renew_control(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        request: Id,
        lease: Id,
        generation: u64,
        now: u64,
    ) -> Result<ControlLease, HostError> {
        self.projection(client, view)?;
        Ok(self
            .sessions
            .renew_control(client, view, request, lease, generation, now)?)
    }
    pub fn release_control(
        &mut self,
        client: &AuthenticatedClient,
        request: Id,
        lease: Id,
        generation: u64,
        now: u64,
    ) -> Result<(), HostError> {
        Ok(self
            .sessions
            .release_control(client, request, lease, generation, now)?)
    }
    pub fn disconnect(&mut self, client: &AuthenticatedClient, now: u64) -> Result<(), HostError> {
        self.sessions.disconnect(client, now)?;
        if let Some(projection) = self.projections.get_mut(&client.client_instance_id) {
            projection.connected = false;
        }
        Ok(())
    }
    pub fn local_activity(&mut self, now: u64) {
        self.sessions.local_activity(now);
    }
    pub fn tick(&mut self, now: u64) {
        self.sessions.tick(now);
        self.ledger.expire_outcomes(now);
    }
    pub fn take_cleanup(&mut self) -> Option<InputCleanup> {
        self.sessions.take_cleanup()
    }
    /// Trusted source operation used to exercise dispatch-time disabled checks.
    pub fn set_counter_enabled(&mut self, enabled: bool, now: u64) -> Result<(), HostError> {
        if self.source.set_counter_enabled(enabled, now)? {
            publish_projections(
                &mut self.sessions,
                &self.source,
                &mut self.projections,
                self.session_id,
                self.epoch,
                now,
            );
        }
        Ok(())
    }
    pub fn view_status(
        &self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<ViewStatus, HostError> {
        let projection = self
            .projections
            .get(&client.client_instance_id)
            .ok_or(SessionError::Unauthenticated)?;
        if projection.client != *client {
            return Err(SessionError::WrongPrincipal.into());
        }
        if projection.view_id != view {
            return Err(SessionError::WrongView.into());
        }
        Ok(ViewStatus {
            revision: projection.revision,
            checkpoint_required: projection.checkpoint_required,
            connected: projection.connected,
            closed: projection.closed,
        })
    }
    pub fn retained_memory_bytes(
        &self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<usize, HostError> {
        self.projection(client, view)?;
        Ok(self.sessions.retained_memory_bytes(client, view)?)
    }
    pub fn diagnostics(&self) -> HostDiagnostics {
        HostDiagnostics {
            source_commit: self.source.commit,
            counter: self.source.counter(),
            clients: self.projections.len(),
            closed_views: self
                .projections
                .values()
                .filter(|p| p.closed.is_some())
                .count(),
            consumed_commands: self.ledger.command_count(),
            retained_command_bytes: self.ledger.retained_command_bytes(),
        }
    }
}
/// A distinct projection scope excludes all visual content and resources before
/// serialization/history. Semantic object IDs and source clocks remain stable.
fn projection_records(records: Vec<p::Record>, projection: &Projection) -> Vec<p::Record> {
    if !projection.semantic_only {
        return records;
    }
    records
        .into_iter()
        .filter_map(|mut record| {
            match record.record.as_mut()? {
                p::record::Record::Surface(surface) => {
                    surface.scene_root = None;
                    surface.visual_coverage = p::VisualCoverage::Unsupported as i32;
                    surface.unsupported_features = vec!["semantic-only-projection".into()];
                }
                p::record::Record::Semantic(node) => {
                    node.visual_bindings.clear();
                    for relation in &mut node.relations {
                        relation.targets.retain(|r| {
                            !matches!(
                                p::ObjectKind::try_from(r.kind),
                                Ok(p::ObjectKind::Scene | p::ObjectKind::Binding)
                            )
                        });
                    }
                }
                p::record::Record::Session(session) => session.cursor = None,
                p::record::Record::Text(_) => {}
                p::record::Record::Scene(_) | p::record::Record::Binding(_) => return None,
            }
            Some(record)
        })
        .collect()
}

fn identity(record: &mut p::Record) -> Result<(u64, p::ObjectKind, &mut u64), HostError> {
    match record.record.as_mut().ok_or(HostError::FixtureLimit)? {
        p::record::Record::Surface(v) => Ok((v.id, p::ObjectKind::Surface, &mut v.version)),
        p::record::Record::Scene(v) => Ok((v.id, p::ObjectKind::Scene, &mut v.version)),
        p::record::Record::Semantic(v) => Ok((v.id, p::ObjectKind::Semantic, &mut v.version)),
        p::record::Record::Text(v) => Ok((v.id, p::ObjectKind::Text, &mut v.version)),
        p::record::Record::Binding(v) => Ok((v.id, p::ObjectKind::Binding, &mut v.version)),
        p::record::Record::Session(v) => Ok((v.id, p::ObjectKind::Session, &mut v.version)),
    }
}
fn capture(
    source: &ControlledApplication,
    projection: &Projection,
    changes: bool,
) -> Result<Capture, HostError> {
    let mut records = projection_records(source.records(projection), projection);
    if records.len() > projection.limits.records {
        return Err(SessionError::Oversized.into());
    }
    let mut stamps = BTreeMap::new();
    let mut mutations = Vec::new();
    for record in &mut records {
        let (id, kind, version) = identity(record)?;
        let source_version = *version;
        let digest: [u8; 32] = Sha256::digest(record.encode_to_vec()).into();
        let old = projection.records.get(&id);
        let changed = old.is_none_or(|stamp| stamp.digest != digest);
        let version = match old {
            None => 1,
            Some(stamp) if !changed => stamp.version,
            Some(stamp) => {
                let next = stamp
                    .version
                    .checked_add(1)
                    .ok_or(HostError::FixtureLimit)?;
                if changes {
                    next
                } else {
                    next.max(source_version)
                }
            }
        };
        *identity(record)?.2 = version;
        if changed && changes {
            mutations.push(p::Mutation {
                mutation: Some(p::mutation::Mutation::Put(record.clone())),
            });
        }
        if stamps
            .insert(
                id,
                RecordStamp {
                    version,
                    digest,
                    kind,
                },
            )
            .is_some()
        {
            return Err(HostError::FixtureLimit);
        }
    }
    for (id, stamp) in &projection.records {
        if !changes {
            break;
        }
        if !stamps.contains_key(id) {
            mutations.push(p::Mutation {
                mutation: Some(p::mutation::Mutation::Delete(fixture::reference(
                    stamp.kind, *id,
                ))),
            });
        }
    }
    Ok(Capture {
        records,
        stamps,
        mutations,
    })
}

fn publish_projections(
    sessions: &mut SessionBroker,
    source: &ControlledApplication,
    projections: &mut BTreeMap<Id, Projection>,
    session_id: Id,
    epoch: Id,
    now: u64,
) {
    for projection in projections.values_mut() {
        if !projection.connected || projection.closed.is_some() || projection.checkpoint_required {
            continue;
        }
        let Some(base) = projection.revision else {
            continue;
        };
        let Some(revision) = base.checked_add(1) else {
            close_projection(
                sessions,
                projection,
                PublicationFailure::RevisionExhausted,
                now,
            );
            continue;
        };
        let _working = match sessions.reserve_view_memory(
            &projection.client,
            projection.view_id,
            source.decoded_projection_bytes(projection),
        ) {
            Ok(memory) => memory,
            Err(_) => {
                close_projection(sessions, projection, PublicationFailure::SlowConsumer, now);
                continue;
            }
        };
        let capture = match capture(source, projection, true) {
            Ok(capture) => capture,
            Err(_) => {
                close_projection(sessions, projection, PublicationFailure::ReceiverLimit, now);
                continue;
            }
        };
        let mutations = capture.mutations;
        if mutations.len() > projection.limits.records {
            close_projection(sessions, projection, PublicationFailure::ReceiverLimit, now);
            continue;
        }
        let transaction = p::Transaction {
            base_revision: base,
            new_revision: revision,
            mutations,
            required_resources: vec![],
            required_features: projection.profiles.iter().cloned().collect(),
        };
        let bytes = projection.envelope(
            session_id,
            epoch,
            p::MessageKind::Transaction,
            transaction.encode_to_vec(),
        );
        if bytes.len() > projection.limits.envelope_bytes {
            close_projection(sessions, projection, PublicationFailure::ReceiverLimit, now);
            continue;
        }
        match sessions.publish(
            &projection.client,
            projection.view_id,
            base,
            revision,
            bytes,
            now,
        ) {
            Ok(()) => {
                projection.revision = Some(revision);
                projection.capture_commit = source.commit;
                projection.counter_clock = source.counter_clock;
                projection.text_clock = source.text_clock;
                projection.records = capture.stamps;
            }
            Err(error) => {
                let reason = match error {
                    SessionError::SlowConsumer => PublicationFailure::SlowConsumer,
                    SessionError::Oversized => PublicationFailure::ReceiverLimit,
                    _ => PublicationFailure::ProjectionUnavailable,
                };
                close_projection(sessions, projection, reason, now);
            }
        }
    }
}
fn close_projection(
    sessions: &mut SessionBroker,
    projection: &mut Projection,
    reason: PublicationFailure,
    now: u64,
) {
    // Explicitly close even a receiver-size error: no invisible revision gap or
    // retrying dependent update can escape the per-client bounded queue.
    let _ = sessions.invalidate_authorization(&projection.client, now);
    projection.closed = Some(reason);
    projection.connected = false;
    projection.checkpoint_required = true;
}
