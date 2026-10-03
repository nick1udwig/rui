//! A controlled, headless RUI host fixture and mandatory command dispatch boundary.
//! Credentials are verified outside this crate. Network/TLS, GUI rendering and OS
//! input injection are not implemented here.
mod corpus;
mod fixture;
mod source_fonts;
mod source_input;
pub use corpus::{
    CANVAS_ID, CANVAS_SHAPE_SCENE_ID, CHECKBOX_ID, ITEM_IDS, LIST_ID, MODAL_TOGGLE_ID,
    POPUP_TOGGLE_ID, PUBLIC_TEXT_ID, PUBLIC_TEXT_MODEL_ID, REORDER_ID, SECOND_SURFACE_ID,
    TOOLTIP_TOGGLE_ID,
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
use rui_protocol_core::resources::{ResourceDigest, ResourceOffer};
use rui_protocol_core::session::{
    ClientHello, ControlLease, InputCleanup, NegotiatedSession, Permissions, PublicationAttachment,
    ReceiverLimits, ResetReason, ResumeDisposition, SessionBroker, SessionConfig, SessionError,
    SharedPayload, VerifiedAuthorization, ViewMemoryReservation, WeakPublicationAttachment,
};
use rui_protocol_core::wire::proto as p;
use sha2::{Digest, Sha256};
pub use source_fonts::{
    NativeSourceFontProvider, SourceFontError, SourceFontProvider, SourceGlyph, SourceOutline,
    SourceShapedText,
};
pub use source_input::{RawSample, FIXTURE_DEVICE, FIXTURE_SEAT, MAX_RAW_SAMPLES};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use thiserror::Error;

/// Trusted authentication adapter's immutable local approval lifetime. Remote
/// packets cannot create, revoke or replace this token. Revocation is permanent
/// and makes even disconnected retained projections unreadable immediately.
#[derive(Clone)]
pub struct AuthorizationLifetime(Arc<AtomicBool>);
impl Default for AuthorizationLifetime {
    fn default() -> Self {
        Self::new()
    }
}
impl AuthorizationLifetime {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(true)))
    }
    pub fn revoke(&self) {
        self.0.store(false, Ordering::Release);
    }
    pub fn is_authorized(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
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
    AuthorizationRevoked,
}
#[derive(Debug, Error, PartialEq, Eq)]
pub enum HostError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error("fixture text or source clock limit exceeded")]
    FixtureLimit,
    #[error("resource is not authorized or retained for this view")]
    ResourceUnavailable,
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
    resources: Option<PublicationAttachment>,
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
    pub fn resources(&self) -> impl Iterator<Item = (&ResourceOffer, &[u8])> {
        self.resources
            .as_ref()
            .and_then(|r| r.get::<ResourceClosure>())
            .into_iter()
            .flat_map(|r| r.outlines.iter())
            .map(|r| (r.offer(), r.bytes()))
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
    resources: Option<PublicationAttachment>,
    resource_registry: BTreeMap<ResourceDigest, Vec<WeakPublicationAttachment>>,
    authorization_lifetime: Option<AuthorizationLifetime>,
    known_resources: BTreeSet<ResourceDigest>,
    resource_knowledge_memory: Option<ViewMemoryReservation>,
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
            resources: None,
            resource_registry: BTreeMap::new(),
            authorization_lifetime: None,
            known_resources: BTreeSet::new(),
            resource_knowledge_memory: None,
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
    input: Option<source_input::SourceInput>,
}
struct ResourceClosure {
    outlines: Vec<SourceOutline>,
}
/// An immutable resource authorized for this current view. Borrowing its offer
/// or body does not escape its shared allocation charge. Debug omits content.
pub struct AuthorizedResource {
    attachment: PublicationAttachment,
    index: usize,
}
impl AuthorizedResource {
    pub fn offer(&self) -> &ResourceOffer {
        self.attachment.get::<ResourceClosure>().unwrap().outlines[self.index].offer()
    }
    pub fn bytes(&self) -> &[u8] {
        self.attachment.get::<ResourceClosure>().unwrap().outlines[self.index].bytes()
    }
}
impl std::fmt::Debug for AuthorizedResource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthorizedResource")
            .field("bytes", &self.bytes().len())
            .finish()
    }
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
            input: None,
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
    /// Source-native public caption projection. The source capability is
    /// trusted local code, never selected or executed by remote clients.
    pub fn new_corpus_with_fonts(
        mut config: HostConfig,
        initial_secret: String,
        provider: Arc<dyn SourceFontProvider>,
    ) -> Result<Self, HostError> {
        config
            .session
            .supported_profiles
            .insert(source_fonts::GLYPH_PROFILE.into());
        let mut host = Self::new_corpus(config, initial_secret)?;
        host.source.fonts = Some(source_fonts::SourceFonts::new(provider));
        host.source.refresh_fonts();
        Ok(host)
    }
    /// Reading a digest does not grant permission. Current registered identity,
    /// policy and view are checked before any retained resource is selected.
    pub fn resource(
        &self,
        client: &AuthenticatedClient,
        view: Id,
        digest: ResourceDigest,
    ) -> Result<AuthorizedResource, HostError> {
        let projection = self.projection(client, view)?;
        let attachment = projection
            .resource_registry
            .get(&digest)
            .and_then(|owners| owners.iter().find_map(WeakPublicationAttachment::upgrade))
            .ok_or(HostError::ResourceUnavailable)?;
        let index = attachment
            .get::<ResourceClosure>()
            .and_then(|r| r.outlines.iter().position(|r| r.offer().digest == digest))
            .ok_or(HostError::ResourceUnavailable)?;
        Ok(AuthorizedResource { attachment, index })
    }
    /// Transport cache bookkeeping only: a previously declared digest permits
    /// reporting an ordinary retired eviction, never reading its offer/body.
    pub fn authorize_resource_eviction(
        &self,
        client: &AuthenticatedClient,
        view: Id,
        digest: ResourceDigest,
    ) -> Result<(), HostError> {
        if self
            .projection(client, view)?
            .known_resources
            .contains(&digest)
        {
            Ok(())
        } else {
            Err(HostError::ResourceUnavailable)
        }
    }
    /// Forget harmless retired-cache declarations after a fully validated batch.
    /// Current source dependencies stay remembered for future cache pressure.
    pub fn acknowledge_retired_resource_evictions(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        digests: &[ResourceDigest],
    ) -> Result<(), HostError> {
        if digests.len() > 512 {
            return Err(HostError::FixtureLimit);
        }
        let projection = self.projection(client, view)?;
        if digests
            .iter()
            .any(|digest| !projection.known_resources.contains(digest))
        {
            return Err(HostError::ResourceUnavailable);
        }
        let projection = self
            .projections
            .get_mut(&client.client_instance_id)
            .unwrap();
        for digest in digests {
            let current = projection
                .resources
                .as_ref()
                .and_then(|resources| resources.get::<ResourceClosure>())
                .is_some_and(|closure| {
                    closure.outlines.iter().any(|r| &r.offer().digest == digest)
                });
            if !current {
                projection.known_resources.remove(digest);
            }
        }
        Ok(())
    }
    pub fn known_resource_count(
        &self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<usize, HostError> {
        Ok(self.projection(client, view)?.known_resources.len())
    }
    /// Trusted transport scratch/writer allocation against the same view ceiling.
    /// Reserve before allocating and retain through buffered socket lifetime.
    pub fn reserve_view_memory(
        &self,
        client: &AuthenticatedClient,
        view: Id,
        bytes: usize,
    ) -> Result<ViewMemoryReservation, HostError> {
        self.projection(client, view)?;
        Ok(self.sessions.reserve_view_memory(client, view, bytes)?)
    }
    /// Select only a resource pinned by this exact publication, while rechecking
    /// the current registered principal/view and originating allocation scope.
    pub fn publication_resource(
        &self,
        client: &AuthenticatedClient,
        view: Id,
        publication: &SharedPayload,
        digest: ResourceDigest,
    ) -> Result<AuthorizedResource, HostError> {
        self.projection(client, view)?;
        let attachment = publication
            .attachment()
            .ok_or(HostError::ResourceUnavailable)?;
        self.sessions
            .authorize_attachment(client, view, attachment)?;
        let index = attachment
            .get::<ResourceClosure>()
            .and_then(|r| r.outlines.iter().position(|r| r.offer().digest == digest))
            .ok_or(HostError::ResourceUnavailable)?;
        Ok(AuthorizedResource {
            attachment: attachment.clone(),
            index,
        })
    }
    pub fn mark_resources_missing(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<(), HostError> {
        self.projection(client, view)?;
        // Neutralize the affected owner while its current readiness still
        // permits clearing common composition state. Observer loss must not
        // cancel another client's input context.
        self.neutralize_input(client, view, 0);
        self.sessions.mark_resources_missing(client, view)?;
        Ok(())
    }
    /// The caller supplies credentials already verified by its trusted auth
    /// adapter. Never deserialize VerifiedAuthorization from client data.
    pub fn negotiate(
        &mut self,
        authorization: VerifiedAuthorization,
        hello: ClientHello,
        now: u64,
    ) -> Result<NegotiatedSession, HostError> {
        self.negotiate_with_lifetime(authorization, hello, None, now)
    }
    pub fn negotiate_with_lifetime(
        &mut self,
        authorization: VerifiedAuthorization,
        hello: ClientHello,
        lifetime: Option<AuthorizationLifetime>,
        now: u64,
    ) -> Result<NegotiatedSession, HostError> {
        if self
            .projections
            .get(&hello.client_instance_id)
            .is_some_and(|old| old.client.principal != authorization.principal)
        {
            return Err(SessionError::WrongPrincipal.into());
        }
        if lifetime
            .as_ref()
            .is_some_and(|token| !token.is_authorized())
        {
            return Err(SessionError::Unauthenticated.into());
        }
        self.prune_revoked_authorizations(now);
        self.synchronize_input(now);
        let old = self
            .projections
            .get(&hello.client_instance_id)
            .map(|projection| (projection.view_id, projection.limits.clone()));
        let requested_resume = hello.resume.is_some();
        let mut negotiated =
            self.sessions
                .negotiate(Some(authorization.clone()), hello.clone(), now)?;
        if requested_resume
            && old.as_ref().is_some_and(|(view, limits)| {
                *view == negotiated.view_id && *limits != negotiated.limits
            })
        {
            self.retire_registered_view(&negotiated.client, now)?;
            let mut fresh = hello;
            fresh.resume = None;
            negotiated = self.sessions.negotiate(Some(authorization), fresh, now)?;
            negotiated.resume = ResumeDisposition::ResetRequired(ResetReason::ViewChanged);
        }

        match self
            .projections
            .get_mut(&negotiated.client.client_instance_id)
        {
            Some(projection) if projection.view_id == negotiated.view_id => {
                projection.connected = true;
                projection.authorization_lifetime = lifetime.clone();
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
                let mut projection = Projection::new(&negotiated);
                projection.authorization_lifetime = lifetime;
                self.projections
                    .insert(negotiated.client.client_instance_id, projection);
            }
        }
        self.synchronize_input(now);
        Ok(negotiated)
    }
    /// Explicit authenticated fresh-view request. Only this registered principal
    /// may retire its producer's prior view; negotiation then creates a distinct
    /// scope and never restores control.
    pub fn retire_registered_view(
        &mut self,
        client: &AuthenticatedClient,
        now: u64,
    ) -> Result<(), HostError> {
        if let Some(projection) = self.projections.get_mut(&client.client_instance_id) {
            if projection.client != *client {
                return Err(SessionError::WrongPrincipal.into());
            }
            close_projection(
                &mut self.sessions,
                projection,
                PublicationFailure::ProjectionUnavailable,
                now,
            );
        }
        self.synchronize_input(now);
        Ok(())
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
        if projection
            .authorization_lifetime
            .as_ref()
            .is_some_and(|token| !token.is_authorized())
        {
            return Err(SessionError::Unauthenticated.into());
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
        let resources =
            resource_closure(&self.sessions, &self.source, projection, &capture.records)?;
        if !can_install_resources(projection, &resources) {
            return Err(SessionError::Oversized.into());
        }
        let knowledge_memory =
            resource_knowledge_reservation(&self.sessions, projection, &resources)?;
        let snapshot = p::Snapshot {
            revision,
            records: capture.records,
            required_resources: resource_digests(&resources),
            required_features: projection.profiles.iter().cloned().collect(),
        };
        let bytes = projection.envelope(
            self.session_id,
            self.epoch,
            p::MessageKind::Snapshot,
            snapshot.encode_to_vec(),
        );
        let output =
            self.sessions
                .snapshot_publication_payload(client, view, bytes, resources.clone())?;
        self.neutralize_input(client, view, 0);
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
        install_resources(projection, resources)?;
        install_resource_knowledge(projection, knowledge_memory);
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
        let output = self.snapshot_publication(client, view)?;
        if output.len() > self.projection(client, view)?.limits.envelope_bytes {
            return Err(SessionError::Oversized.into());
        }
        Ok(output)
    }
    /// Read-only whole snapshot/closure keeper for a chunked authenticated
    /// transport. This does not reset history or acknowledgements.
    pub fn snapshot_publication(
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
        let capture = capture(&self.source, projection, false)?;
        let resources =
            resource_closure(&self.sessions, &self.source, projection, &capture.records)?;
        let snapshot = p::Snapshot {
            revision,
            records: capture.records,
            required_resources: resource_digests(&resources),
            required_features: projection.profiles.iter().cloned().collect(),
        };
        let bytes = projection.envelope(
            self.session_id,
            self.epoch,
            p::MessageKind::Snapshot,
            snapshot.encode_to_vec(),
        );
        Ok(self
            .sessions
            .snapshot_publication_payload(client, view, bytes, resources)?)
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
            resources: projection.resources.clone(),
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
            } else if self.source.corpus.is_some()
                && !(projection.profiles.contains(source_fonts::GLYPH_PROFILE)
                    && self.source.fonts.as_ref().is_some_and(|f| f.complete()))
            {
                "partial"
            } else {
                "complete"
            }
            .into(),
            input_modes: if projection.permissions.can_control {
                let mut modes = vec!["semantic.v1".into()];
                if self.source.corpus.is_some() && !projection.semantic_only {
                    for (permission, mode) in [
                        ("RawInputBatch", "raw.pointer.v1"),
                        ("TextComposition", "text.client.v1"),
                        ("ResizeRequest", "resize.v1"),
                    ] {
                        if projection.permissions.actions.contains(permission) {
                            modes.push(mode.into());
                        }
                    }
                }
                modes
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
        self.synchronize_input(now);
        let presented = self
            .sessions
            .presented_revision(client, view)
            .ok()
            .flatten();
        let source = RefCell::new(&mut self.source);
        let projections = RefCell::new(&mut self.projections);
        let input = RefCell::new(&mut self.input);
        let session_id = self.session_id;
        let epoch = self.epoch;
        let result = self.ledger.submit_controlled_with_session(
            &mut self.sessions,
            client,
            family,
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
                let source = source.borrow();
                if command.header().observed_view_revision > projection.revision.unwrap_or(0) {
                    return Err(Rejection::FutureRevision);
                }
                match command {
                    Command::Semantic(c) => source.validate(projection, c),
                    Command::RawInput(c) => {
                        let corpus = source.corpus.as_ref().ok_or(Rejection::UnsupportedAction)?;
                        if c.batch.presented_revision != presented.unwrap_or(0)
                            || c.batch.presented_revision != c.header.observed_view_revision
                        {
                            return Err(Rejection::StaleTarget);
                        }
                        source
                            .commit
                            .checked_add(1)
                            .ok_or(Rejection::InvalidArguments)?;
                        input
                            .borrow()
                            .as_ref()
                            .ok_or(Rejection::Unauthorized)?
                            .validate_raw(c, &corpus.raw_route(presented.unwrap_or(0)))
                    }
                    Command::TextComposition(c) => {
                        let context = source
                            .editing_context(projection)
                            .ok_or(Rejection::Unauthorized)?;
                        if context.target != c.target_id
                            || context.focus_generation != c.focus_generation
                        {
                            return Err(Rejection::StaleFocus);
                        }
                        if context.text_version != c.base_text_version {
                            return Err(Rejection::StaleText);
                        }
                        let owner = input.borrow();
                        let owner = owner.as_ref().ok_or(Rejection::Unauthorized)?;
                        owner.validate_composition(c)?;
                        if let rui_protocol_core::commands::CompositionOperation::CommitText {
                            text,
                        } = &c.operation
                        {
                            source.validate_composed_edit(
                                projection,
                                c.target_id,
                                c.focus_generation,
                                c.base_text_version,
                                owner.range().ok_or(Rejection::StaleText)?,
                                text,
                            )?;
                        }
                        Ok(())
                    }
                    Command::Resize(c) => {
                        let corpus = source.corpus.as_ref().ok_or(Rejection::UnsupportedAction)?;
                        if c.surface_id != SURFACE_ID {
                            return Err(Rejection::MissingTarget);
                        }
                        if projection.records.get(&SURFACE_ID).map(|s| s.version)
                            != Some(c.surface_version)
                        {
                            return Err(Rejection::StaleTarget);
                        }
                        source
                            .commit
                            .checked_add(1)
                            .ok_or(Rejection::InvalidArguments)?;
                        corpus.validate_resize(c.width, c.height)
                    }
                }
            },
            |command, sessions| {
                let commit = {
                    let all = projections.borrow();
                    let projection = all
                        .get(&client.client_instance_id)
                        .ok_or(DispatchFailure::Failed)?;
                    let mut source = source.borrow_mut();
                    match command {
                        Command::Semantic(c) => source
                            .apply(projection, c, now)
                            .map_err(|_| DispatchFailure::Failed)?,
                        Command::RawInput(c) => {
                            let commit = source
                                .commit
                                .checked_add(1)
                                .ok_or(DispatchFailure::Failed)?;
                            let corpus = source.corpus.as_mut().ok_or(DispatchFailure::Failed)?;
                            let route = corpus.raw_route(presented.unwrap_or(0));
                            input
                                .borrow_mut()
                                .as_mut()
                                .ok_or(DispatchFailure::Failed)?
                                .dispatch_raw(c, &route, sessions, now, |event| {
                                    corpus.apply_raw_event(event)
                                })
                                .map_err(|error| {
                                    if error == rui_protocol_core::input::InputError::OutcomeUnknown
                                    {
                                        DispatchFailure::OutcomeUnknown
                                    } else {
                                        DispatchFailure::Failed
                                    }
                                })?;
                            source.finish_input_commit(commit, now);
                            commit
                        }
                        Command::TextComposition(c) => {
                            let edit = input
                                .borrow_mut()
                                .as_mut()
                                .ok_or(DispatchFailure::Failed)?
                                .compose(c, sessions, now)
                                .map_err(|_| DispatchFailure::OutcomeUnknown)?;
                            let Some(edit) = edit else {
                                return Ok(DispatchOutcome::Dispatched);
                            };
                            source
                                .apply_composed_edit(projection, &edit, now)
                                .map_err(|_| DispatchFailure::OutcomeUnknown)?
                        }
                        Command::Resize(c) => {
                            let commit = source
                                .commit
                                .checked_add(1)
                                .ok_or(DispatchFailure::Failed)?;
                            source
                                .corpus
                                .as_mut()
                                .ok_or(DispatchFailure::Failed)?
                                .resize(c.width, c.height)
                                .map_err(|_| DispatchFailure::Failed)?;
                            source.finish_input_commit(commit, now);
                            commit
                        }
                    }
                };
                {
                    let all = projections.borrow();
                    synchronize_source_input(
                        &mut input.borrow_mut(),
                        sessions,
                        &mut source.borrow_mut(),
                        all.get(&client.client_instance_id),
                        now,
                    );
                }
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
    /// A trusted network sender retains the exact broker-created transaction
    /// through resource verification, even when bounded history has moved on.
    pub fn acknowledge_applied_publication(
        &mut self,
        client: &AuthenticatedClient,
        view: Id,
        revision: u64,
        publication: &SharedPayload,
    ) -> Result<(), HostError> {
        self.projection(client, view)?;
        self.sessions
            .acknowledge_applied_publication(client, view, revision, true, publication)?;
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
        self.synchronize_input(now);
        if self.projection(client, view)?.checkpoint_required {
            return Err(HostError::CheckpointRequired);
        }
        let projection = self.projection(client, view)?;
        let needs_input = self.source.corpus.is_some()
            && !projection.semantic_only
            && ["RawInputBatch", "TextComposition"]
                .iter()
                .any(|action| projection.permissions.actions.contains(*action));
        let memory = if needs_input && self.input.is_none() {
            Some(self.sessions.reserve_view_memory(
                client,
                view,
                source_input::SourceInput::memory_bytes(self.source.max_text_bytes()),
            )?)
        } else {
            None
        };
        let lease = self.sessions.acquire_control(client, view, request, now)?;
        if let Some(memory) = memory {
            self.input = Some(source_input::SourceInput::new(
                lease.clone(),
                self.source.max_text_bytes(),
                memory,
            ));
        }
        self.synchronize_input(now);
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
        self.sessions
            .release_control(client, request, lease, generation, now)?;
        self.synchronize_input(now);
        Ok(())
    }
    pub fn disconnect(&mut self, client: &AuthenticatedClient, now: u64) -> Result<(), HostError> {
        self.sessions.disconnect(client, now)?;
        if let Some(projection) = self.projections.get_mut(&client.client_instance_id) {
            projection.connected = false;
        }
        self.synchronize_input(now);
        Ok(())
    }
    pub fn local_activity(&mut self, now: u64) {
        self.sessions.local_activity(now);
        self.synchronize_input(now);
    }
    pub fn tick(&mut self, now: u64) {
        self.prune_revoked_authorizations(now);
        self.sessions.tick(now);
        self.synchronize_input(now);
        self.ledger.expire_outcomes(now);
    }
    fn prune_revoked_authorizations(&mut self, now: u64) {
        for projection in self.projections.values_mut() {
            if projection
                .authorization_lifetime
                .as_ref()
                .is_some_and(|token| !token.is_authorized())
                && projection.closed.is_none()
            {
                close_projection(
                    &mut self.sessions,
                    projection,
                    PublicationFailure::AuthorizationRevoked,
                    now,
                );
            }
        }
    }
    pub fn take_cleanup(&mut self) -> Option<InputCleanup> {
        self.sessions.take_cleanup()
    }
    fn synchronize_input(&mut self, now: u64) {
        let projection = self
            .input
            .as_ref()
            .and_then(|input| self.projections.get(&input.lease.client.client_instance_id));
        synchronize_source_input(
            &mut self.input,
            &mut self.sessions,
            &mut self.source,
            projection,
            now,
        );
    }
    fn neutralize_input(&mut self, client: &AuthenticatedClient, view: Id, now: u64) {
        if let Some(owner) = self
            .input
            .as_mut()
            .filter(|owner| owner.lease.client == *client && owner.lease.view_id == view)
        {
            owner.cleanup(&mut self.sessions, now);
            if let Some(corpus) = &mut self.source.corpus {
                corpus.cancel_drag();
            }
        }
    }
    pub fn input_trace(
        &self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<&[RawSample], HostError> {
        self.projection(client, view)?;
        let Some(input) = &self.input else {
            return Ok(&[]);
        };
        if input.lease.client != *client || input.lease.view_id != view {
            return Err(SessionError::Unauthorized.into());
        }
        Ok(input.samples())
    }
    /// Trusted source inspection only. Preedit is never included in publications,
    /// recovery history, observer diagnostics or command results.
    pub fn input_preedit(
        &self,
        client: &AuthenticatedClient,
        view: Id,
    ) -> Result<Option<&str>, HostError> {
        self.projection(client, view)?;
        let Some(input) = &self.input else {
            return Ok(None);
        };
        if input.lease.client != *client || input.lease.view_id != view {
            return Err(SessionError::Unauthorized.into());
        }
        Ok(input.preedit())
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
        if projection
            .authorization_lifetime
            .as_ref()
            .is_some_and(|token| !token.is_authorized())
        {
            close_projection(
                sessions,
                projection,
                PublicationFailure::AuthorizationRevoked,
                now,
            );
            continue;
        }
        if projection.closed.is_some() || projection.checkpoint_required {
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
        let _working = match sessions.reserve_producer_view_memory(
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
        let resources = match resource_closure(sessions, source, projection, &capture.records) {
            Ok(resources) => resources,
            Err(_) => {
                close_projection(sessions, projection, PublicationFailure::SlowConsumer, now);
                continue;
            }
        };
        if !can_install_resources(projection, &resources) {
            close_projection(sessions, projection, PublicationFailure::ReceiverLimit, now);
            continue;
        }
        let knowledge_memory =
            match resource_knowledge_reservation(sessions, projection, &resources) {
                Ok(memory) => memory,
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
            required_resources: resource_digests(&resources),
            required_features: projection.profiles.iter().cloned().collect(),
        };
        let bytes = projection.envelope(
            session_id,
            epoch,
            p::MessageKind::Transaction,
            transaction.encode_to_vec(),
        );
        match sessions.publish_retained_with_attachment(
            &projection.client,
            projection.view_id,
            base,
            revision,
            bytes,
            resources.clone(),
            now,
        ) {
            Ok(()) => {
                projection.revision = Some(revision);
                projection.capture_commit = source.commit;
                projection.counter_clock = source.counter_clock;
                projection.text_clock = source.text_clock;
                projection.records = capture.stamps;
                let _ = install_resources(projection, resources);
                install_resource_knowledge(projection, knowledge_memory);
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
fn synchronize_source_input(
    input: &mut Option<source_input::SourceInput>,
    sessions: &mut SessionBroker,
    source: &mut ControlledApplication,
    projection: Option<&Projection>,
    now: u64,
) {
    let current = sessions.current_lease(now);
    let valid = input.as_ref().is_some_and(|owner| {
        current.as_ref().is_some_and(|lease| {
            lease.lease_id == owner.lease.lease_id
                && lease.generation == owner.lease.generation
                && lease.client == owner.lease.client
                && lease.view_id == owner.lease.view_id
        }) && projection.is_some_and(|projection| {
            projection.connected
                && projection.closed.is_none()
                && projection
                    .authorization_lifetime
                    .as_ref()
                    .is_none_or(AuthorizationLifetime::is_authorized)
        })
    });
    if !valid {
        if let Some(mut owner) = input.take() {
            owner.cleanup(sessions, now);
            if let Some(corpus) = &mut source.corpus {
                corpus.cancel_drag();
            }
        }
        return;
    }
    let owner = input.as_mut().unwrap();
    if sessions
        .presented_revision(&owner.lease.client, owner.lease.view_id)
        .ok()
        .flatten()
        .is_none()
    {
        owner.cleanup(sessions, now);
        if let Some(corpus) = &mut source.corpus {
            corpus.cancel_drag();
        }
        return;
    }
    let Some(corpus) = source.corpus.as_ref() else {
        return;
    };
    let token = corpus.routing_token();
    let changed = input
        .as_ref()
        .unwrap()
        .source_token()
        .is_some_and(|old| old != token);
    let context = source.editing_context(projection.unwrap());
    input
        .as_mut()
        .unwrap()
        .synchronize(context, token, sessions, now);
    if changed {
        source.corpus.as_mut().unwrap().cancel_drag();
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
    projection.resources = None;
    projection.resource_registry.clear();
    projection.known_resources.clear();
    projection.resource_knowledge_memory = None;
}

const MAX_KNOWN_RESOURCES: usize = 4096;
const RESOURCE_KNOWLEDGE_BYTES: usize = MAX_KNOWN_RESOURCES * 128;
fn resource_knowledge_reservation(
    sessions: &SessionBroker,
    projection: &Projection,
    resources: &Option<PublicationAttachment>,
) -> Result<Option<ViewMemoryReservation>, HostError> {
    let Some(closure) = resources.as_ref().and_then(|r| r.get::<ResourceClosure>()) else {
        return Ok(None);
    };
    let added = closure
        .outlines
        .iter()
        .filter(|r| !projection.known_resources.contains(&r.offer().digest))
        .count();
    if projection.known_resources.len() + added > MAX_KNOWN_RESOURCES {
        return Err(HostError::FixtureLimit);
    }
    if projection.resource_knowledge_memory.is_some() {
        return Ok(None);
    }
    // Conservative BTreeSet metadata ceiling; reserve before inserting any IDs.
    Ok(Some(sessions.reserve_producer_view_memory(
        &projection.client,
        projection.view_id,
        RESOURCE_KNOWLEDGE_BYTES,
    )?))
}
fn install_resource_knowledge(projection: &mut Projection, memory: Option<ViewMemoryReservation>) {
    if let Some(memory) = memory {
        projection.resource_knowledge_memory = Some(memory);
    }
    if let Some(closure) = projection
        .resources
        .as_ref()
        .and_then(|r| r.get::<ResourceClosure>())
    {
        for outline in &closure.outlines {
            projection.known_resources.insert(outline.offer().digest);
        }
    }
}

fn resource_digests(resources: &Option<PublicationAttachment>) -> Vec<Vec<u8>> {
    resources
        .as_ref()
        .and_then(|r| r.get::<ResourceClosure>())
        .map_or_else(Vec::new, |r| {
            r.outlines
                .iter()
                .map(|r| r.offer().digest.to_vec())
                .collect()
        })
}
fn resource_closure(
    sessions: &SessionBroker,
    source: &ControlledApplication,
    projection: &Projection,
    records: &[p::Record],
) -> Result<Option<PublicationAttachment>, HostError> {
    let digests: BTreeSet<ResourceDigest> = records
        .iter()
        .filter_map(|r| match &r.record {
            Some(p::record::Record::Scene(s)) => Some(&s.resource_refs),
            _ => None,
        })
        .flatten()
        .map(|d| d.as_slice().try_into().map_err(|_| HostError::FixtureLimit))
        .collect::<Result<_, _>>()?;
    if digests.is_empty() {
        return Ok(None);
    }
    if let Some(existing) = &projection.resources {
        if existing.get::<ResourceClosure>().is_some_and(|r| {
            r.outlines
                .iter()
                .map(|r| r.offer().digest)
                .eq(digests.iter().copied())
        }) {
            return Ok(Some(existing.clone()));
        }
    }
    let available = source.source_resources(projection);
    let mut bytes = 512usize;
    for digest in &digests {
        let r = available
            .iter()
            .find(|r| r.offer().digest == *digest)
            .ok_or(HostError::ResourceUnavailable)?;
        if r.bytes().len() > projection.limits.resource_bytes {
            return Err(SessionError::Oversized.into());
        }
        bytes = bytes
            .checked_add(r.bytes().len() + 512)
            .ok_or(HostError::FixtureLimit)?;
    }
    let memory =
        sessions.reserve_producer_view_memory(&projection.client, projection.view_id, bytes)?;
    let outlines = digests
        .iter()
        .map(|digest| {
            available
                .iter()
                .find(|r| r.offer().digest == *digest)
                .unwrap()
                .clone()
        })
        .collect();
    Ok(Some(memory.attach(ResourceClosure { outlines })))
}
fn can_install_resources(
    projection: &Projection,
    resources: &Option<PublicationAttachment>,
) -> bool {
    let mut counts: BTreeMap<_, usize> = projection
        .resource_registry
        .iter()
        .filter_map(|(d, owners)| {
            let live = owners.iter().filter(|v| v.upgrade().is_some()).count();
            (live > 0).then_some((*d, live))
        })
        .collect();
    if let Some(resources) = resources {
        for outline in &resources.get::<ResourceClosure>().unwrap().outlines {
            let existing = projection
                .resource_registry
                .get(&outline.offer().digest)
                .is_some_and(|owners| {
                    owners
                        .iter()
                        .filter_map(WeakPublicationAttachment::upgrade)
                        .any(|v| v.ptr_eq(resources))
                });
            if !existing {
                *counts.entry(outline.offer().digest).or_default() += 1;
            }
        }
    }
    counts.len() <= 512 && counts.values().sum::<usize>() <= 4096
}
fn install_resources(
    projection: &mut Projection,
    resources: Option<PublicationAttachment>,
) -> Result<(), HostError> {
    if !can_install_resources(projection, &resources) {
        return Err(SessionError::Oversized.into());
    }
    // Weak allocation metadata itself has independent finite ceilings (512
    // digests,4096 owners) covered by the live decoded projection reservation.
    projection.resource_registry.retain(|_, owners| {
        owners.retain(|v| v.upgrade().is_some());
        !owners.is_empty()
    });
    if let Some(resources) = &resources {
        for outline in &resources.get::<ResourceClosure>().unwrap().outlines {
            let owners = projection
                .resource_registry
                .entry(outline.offer().digest)
                .or_default();
            if !owners
                .iter()
                .filter_map(WeakPublicationAttachment::upgrade)
                .any(|v| v.ptr_eq(resources))
            {
                owners.push(resources.downgrade());
            }
        }
    }
    projection.resources = resources;
    Ok(())
}
