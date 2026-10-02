//! A controlled, headless RUI host fixture and mandatory command dispatch boundary.
//! Credentials are verified outside this crate. Network/TLS, GUI rendering and OS
//! input injection are not implemented here.
mod fixture;
use fixture::{ControlledApplication, RECORD_COUNT};
pub use fixture::{
    COUNTER_ID, COUNTER_SCENE_ID, SECRET_ID, SECRET_SCENE_ID, SECRET_TEXT_ID, SURFACE_ID,
};
use prost::Message;
use rui_protocol_core::commands::{
    AuthenticatedClient, CommandLedger, CommandResult, DispatchFailure, DispatchOutcome, Id,
    LedgerError, LedgerLimits, Rejection, SourceEvidence,
};
use rui_protocol_core::session::{
    ClientHello, ControlLease, InputCleanup, NegotiatedSession, Permissions, ReceiverLimits,
    ResetReason, ResumeDisposition, SessionBroker, SessionConfig, SessionError, SharedPayload,
    VerifiedAuthorization,
};
use rui_protocol_core::wire::proto as p;
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostDiagnostics {
    pub source_commit: u64,
    pub counter: i64,
    pub clients: usize,
    pub closed_views: usize,
    pub consumed_commands: usize,
    pub retained_command_bytes: usize,
}
struct Projection {
    client: AuthenticatedClient,
    view_id: Id,
    version: u32,
    profiles: BTreeSet<String>,
    limits: ReceiverLimits,
    permissions: Permissions,
    revision: Option<u64>,
    capture_commit: u64,
    counter_clock: u64,
    text_clock: u64,
    connected: bool,
    checkpoint_required: bool,
    closed: Option<PublicationFailure>,
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
            revision: None,
            capture_commit: 0,
            counter_clock: 0,
            text_clock: 0,
            connected: true,
            checkpoint_required: true,
            closed: None,
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
    pub fn new(config: HostConfig, initial_secret: String) -> Result<Self, HostError> {
        Ok(Self {
            session_id: config.session_id,
            epoch: config.epoch,
            sessions: SessionBroker::new(config.session_id, config.epoch, config.session)?,
            ledger: CommandLedger::new(config.epoch, config.ledger)?,
            source: ControlledApplication::new(initial_secret, config.max_text_bytes)?,
            projections: BTreeMap::new(),
        })
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
        if projection.limits.records < RECORD_COUNT {
            return Err(SessionError::Oversized.into());
        }
        let revision = projection
            .revision
            .map_or(Some(1), |r| r.checked_add(1))
            .ok_or(HostError::FixtureLimit)?;
        let snapshot = p::Snapshot {
            revision,
            records: self.source.records(projection),
            required_resources: vec![],
            required_features: projection.profiles.iter().cloned().collect(),
        };
        let bytes = projection.envelope(
            self.session_id,
            self.epoch,
            p::MessageKind::Snapshot,
            snapshot.encode_to_vec(),
        );
        let decoded = self.source.decoded_projection_bytes(projection);
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
        let snapshot = p::Snapshot {
            revision,
            records: self.source.records(projection),
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
    /// Exactly one ledger admission consumes well-formed rejections. Mandatory
    /// current auth/lease/source validation is inside that admission. Only host
    /// code executes the business operation, followed by independent projections.
    pub fn submit(
        &mut self,
        client: &AuthenticatedClient,
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
        let sessions = RefCell::new(&mut self.sessions);
        let source = RefCell::new(&mut self.source);
        let projections = RefCell::new(&mut self.projections);
        let session_id = self.session_id;
        let epoch = self.epoch;
        let result = self.ledger.submit(
            client,
            raw_command,
            now,
            |command| {
                let all = projections.borrow();
                let projection = all
                    .get(&client.client_instance_id)
                    .ok_or(Rejection::Unauthorized)?;
                if projection.checkpoint_required {
                    return Err(Rejection::WrongView);
                }
                sessions
                    .borrow_mut()
                    .validate_control(client, command, now)?;
                source.borrow().validate(projection, command)
            },
            |command| {
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
                    &mut sessions.borrow_mut(),
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
        let mutations: Vec<_> = source
            .changed_records(projection)
            .into_iter()
            .map(|record| p::Mutation {
                mutation: Some(p::mutation::Mutation::Put(record)),
            })
            .collect();
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
