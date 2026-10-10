//! A controlled cooperating GTK source behind the common session/command gate.
//! The caller authenticates principals. The inherited native socket is private
//! to this host and its source; it accepts only commands this ledger admitted.
//! Public projections have independent bounded delivery and resource ownership.
use crate::native_fence::NativeFence;
use crate::source_identity::SourceIdentities;
use crate::{
    bounded_read, manifest_preflight, CaptureArtifact, CaptureError, MAX_ENVELOPE_BYTES,
    MAX_MANIFEST_ALLOCATION,
};
use prost::Message;
use rui_protocol_core::commands::{
    Arguments, AuthenticatedClient, Command, CommandFamily, CommandLedger, CommandResult,
    CompositionOperation, DispatchFailure, DispatchOutcome, Id, LedgerError, LedgerLimits,
    Rejection, SemanticAuthority, SemanticCommand, SourceEvidence, TargetState,
    TextCompositionCommand,
};
use rui_protocol_core::model::{
    Limits, Record, SemanticRole, Sensitivity, State, StateKey, TextStatus, TriState,
};
use rui_protocol_core::receiver::{ReceiveOutcome, ReceiverConfig, ViewReceiver};
use rui_protocol_core::resources::{
    MemoryBudget, MemoryError, MemoryReservation, OfferOutcome, ResourceContext, ResourceDigest,
    ResourceOffer, ResourceScope,
};
use rui_protocol_core::session::{
    ClientHello, ControlLease, NegotiatedSession, ReceiverLimits, SessionBroker, SessionConfig,
    SessionError, SharedPayload, VerifiedAuthorization, ViewMemoryReservation,
};
use rui_protocol_core::wire::{self, proto as pb};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;

#[path = "gtk_composition.rs"]
mod composition;

#[cfg(feature = "transport")]
#[path = "gtk_port.rs"]
mod port;
#[cfg(feature = "transport")]
pub use port::GtkProducerPort;

const MAX_REQUEST: usize = 128 << 10;
const MAX_REPLY: usize = 8 << 10;
const SOURCE_MEMORY: usize = 96 << 20;
const CLIENT_MEMORY: usize = 8 << 20;
// A normal full-corpus mirror viewport retains the old GTK texture while
// allocating a linear frame, its geometric plan and the new RGBA texture.
// Actual1024x640 presentation exceeded32MiB;64MiB keeps this bounded overlap.
const PUBLIC_VIEW_MEMORY: usize = 64 << 20;
const ARTIFACT_MEMORY: usize = MAX_MANIFEST_ALLOCATION;
// One 16MiB body plus bounded resource descriptor/seen sets and wire scratch.
// Source resource bodies are also independently charged inside the receiver.
const CAPTURE_WORKING_MEMORY: usize = 17 << 20;
const PROJECT_MEMORY: usize = MAX_MANIFEST_ALLOCATION + 3 * MAX_ENVELOPE_BYTES as usize;
const MAX_CLIENTS: usize = 4;
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum GtkHostError {
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Memory(#[from] MemoryError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error("private native source pipe failed or exceeded its deadline")]
    NativePipe,
    #[error("native source capture/command is unsupported in this public semantic fixture")]
    Unsupported,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct NativeDiagnostics {
    pub native_dispatches: u64,
    pub native_callbacks: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    request_id: u64,
    source_generation: u64,
    status: String,
    artifact: Option<String>,
    reason: Option<String>,
    native_dispatches: u64,
    native_callbacks: u64,
}
struct NativeCapture {
    artifact: CaptureArtifact,
    _memory: MemoryReservation,
}
struct NativePipe {
    fence: NativeFence,
    child: Child,
    socket: UnixStream,
    directory: PathBuf,
    budget: MemoryBudget,
    _working: MemoryReservation,
    next_request: u64,
    source_generation: u64,
    failed: bool,
    diagnostics: NativeDiagnostics,
}
impl Drop for NativePipe {
    fn drop(&mut self) {
        self.fence.invalidate();
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.purge_artifacts();
    }
}
impl NativePipe {
    fn purge_artifacts(&self) -> Result<(), GtkHostError> {
        // This dedicated directory was empty before spawning our owned source.
        // Walk without retaining an uncharged file inventory. No nested paths
        // or symlinks are accepted from the cooperating source.
        let mut count = 0usize;
        let mut bytes = 0u64;
        for entry in std::fs::read_dir(&self.directory)? {
            let entry = entry?;
            count += 1;
            let metadata = entry.metadata()?;
            bytes = bytes
                .checked_add(metadata.len())
                .ok_or(GtkHostError::Unsupported)?;
            if count > 16384 || bytes > 128 << 20 || !entry.file_type()?.is_file() {
                return Err(GtkHostError::Unsupported);
            }
        }
        for entry in std::fs::read_dir(&self.directory)? {
            std::fs::remove_file(entry?.path())?;
        }
        Ok(())
    }
    fn acknowledge_capture(&mut self, capture: &NativeCapture) -> Result<(), GtkHostError> {
        #[derive(Serialize)]
        struct Consumed<'a> {
            request_id: u64,
            kind: &'static str,
            artifact: &'a str,
        }
        let artifact = capture
            .artifact
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(GtkHostError::NativePipe)?;
        let reply = self.request(&Consumed {
            request_id: self.next_request,
            kind: "consumed",
            artifact,
        })?;
        if reply.status != "consumed" {
            return Err(GtkHostError::NativePipe);
        }
        Ok(())
    }
    fn spawn(
        mut command: ProcessCommand,
        directory: PathBuf,
        budget: MemoryBudget,
        source_generation: u64,
    ) -> Result<(Self, NativeCapture), GtkHostError> {
        // Covers encoded request, read buffer, decoded bounded reply and spare
        // serde containers throughout the one permitted round trip.
        let working = budget.reserve(1 << 20)?;
        let fence = NativeFence::new(&budget, source_generation)?;
        fence.inherit(&mut command);
        let (socket, child_socket) = UnixStream::pair()?;
        command
            .env(
                "RUI_PRIVATE_SOURCE_GENERATION",
                source_generation.to_string(),
            )
            .stdin(Stdio::from(OwnedFd::from(child_socket)))
            .stdout(Stdio::null());
        let child = command.spawn()?;
        let mut pipe = Self {
            fence,
            child,
            socket,
            directory,
            budget,
            _working: working,
            next_request: 1,
            source_generation,
            failed: false,
            diagnostics: NativeDiagnostics::default(),
        };
        let reply = pipe.read_reply(0, Instant::now() + TIMEOUT)?;
        if reply.status != "ready" {
            return Err(GtkHostError::NativePipe);
        }
        let capture = pipe.capture(reply)?;
        Ok((pipe, capture))
    }
    fn capture(&self, reply: Reply) -> Result<NativeCapture, GtkHostError> {
        self.capture_reserved(reply, self.budget.reserve(ARTIFACT_MEMORY)?)
    }
    fn capture_reserved(
        &self,
        reply: Reply,
        memory: MemoryReservation,
    ) -> Result<NativeCapture, GtkHostError> {
        let name = reply.artifact.ok_or(GtkHostError::NativePipe)?;
        if name.len() > 256
            || name.contains('/')
            || name.contains('\\')
            || !name.ends_with(".json")
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            return Err(GtkHostError::NativePipe);
        }
        // Pending and current captures have separate reservations. Reserve
        // before the bounded reader/serde allocates any capture metadata.
        let artifact = CaptureArtifact::read(self.directory.join(name))?;
        if artifact.manifest()["source_generation"].as_u64() != Some(self.source_generation) {
            return Err(GtkHostError::NativePipe);
        }
        Ok(NativeCapture {
            artifact,
            _memory: memory,
        })
    }
    fn transfer(
        &mut self,
        bytes: &mut [u8],
        write: bool,
        deadline: Instant,
    ) -> Result<(), GtkHostError> {
        let mut offset = 0;
        while offset < bytes.len() {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .filter(|duration| !duration.is_zero())
                .ok_or(GtkHostError::NativePipe)?;
            if write {
                self.socket.set_write_timeout(Some(remaining))?;
            } else {
                self.socket.set_read_timeout(Some(remaining))?;
            }
            let result = if write {
                self.socket.write(&bytes[offset..])
            } else {
                self.socket.read(&mut bytes[offset..])
            };
            let count = match result {
                Ok(0) => return Err(GtkHostError::NativePipe),
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(GtkHostError::NativePipe),
            };
            offset += count;
        }
        Ok(())
    }
    fn read_reply(&mut self, request: u64, deadline: Instant) -> Result<Reply, GtkHostError> {
        let mut header = [0; 4];
        self.transfer(&mut header, false, deadline)?;
        let length = u32::from_be_bytes(header) as usize;
        if length == 0 || length > MAX_REPLY {
            return Err(GtkHostError::NativePipe);
        }
        let mut bytes = vec![0; length];
        self.transfer(&mut bytes, false, deadline)?;
        manifest_preflight(&bytes)?;
        let reply: Reply = serde_json::from_slice(&bytes).map_err(|_| GtkHostError::NativePipe)?;
        if reply.request_id != request
            || reply.source_generation != self.source_generation
            || reply
                .reason
                .as_ref()
                .is_some_and(|reason| reason.len() > 256)
            || reply.status.len() > 32
        {
            return Err(GtkHostError::NativePipe);
        }
        self.diagnostics = NativeDiagnostics {
            native_dispatches: reply.native_dispatches,
            native_callbacks: reply.native_callbacks,
        };
        Ok(reply)
    }
    fn request<T: Serialize>(&mut self, request: &T) -> Result<Reply, GtkHostError> {
        if self.failed {
            return Err(GtkHostError::NativePipe);
        }
        let outcome = (|| {
            let mut bytes = serde_json::to_vec(request).map_err(|_| GtkHostError::NativePipe)?;
            if bytes.is_empty() || bytes.len() > MAX_REQUEST {
                return Err(GtkHostError::NativePipe);
            }
            let deadline = Instant::now() + TIMEOUT;
            self.transfer(&mut (bytes.len() as u32).to_be_bytes(), true, deadline)?;
            self.transfer(&mut bytes, true, deadline)?;
            self.read_reply(self.next_request, deadline)
        })();
        if outcome.is_err() {
            self.failed = true;
        }
        self.next_request = self
            .next_request
            .checked_add(1)
            .ok_or(GtkHostError::NativePipe)?;
        outcome
    }
}

#[derive(Serialize)]
struct SourceGuard {
    source_generation: u64,
    incarnation: String,
    surface_id: u64,
    target_id: u64,
    target_version: u64,
    text_version: Option<u64>,
    model_version: Option<u64>,
    focus_generation: Option<u64>,
    lease_id: String,
    control_generation: u64,
    deadline_ns: u64,
    text_record_id: Option<u64>,
    text_record_version: Option<u64>,
    selection: Option<(u64, u64)>,
}
#[derive(Serialize)]
#[serde(untagged)]
enum NativeArguments<'a> {
    None {},
    Text { start: u64, end: u64, text: &'a str },
    Selection { anchor: u64, focus: u64 },
    Scroll { x: f64, y: f64 },
}
#[derive(Serialize)]
struct NativeCommand<'a> {
    request_id: u64,
    kind: &'static str,
    guard: SourceGuard,
    action: &'a str,
    arguments: NativeArguments<'a>,
}
#[derive(Serialize)]
struct CaptureRequest {
    request_id: u64,
    kind: &'static str,
}

#[derive(Serialize)]
struct NativeComposition<'a> {
    request_id: u64,
    kind: &'static str,
    guard: SourceGuard,
    text_session_id: String,
    stage: &'static str,
    range: Option<(u64, u64)>,
    text: Option<&'a str>,
    selection: Option<(u64, u64)>,
}
#[derive(Serialize)]
#[serde(untagged)]
enum NativeRequest<'a> {
    Semantic(NativeCommand<'a>),
    Composition(NativeComposition<'a>),
}

struct View {
    negotiated: NegotiatedSession,
    // This controlled fixture contains an actual bounded semantic consumer.
    // Its state is private; readiness is set only after it decodes a publication.
    consumer: ViewReceiver,
    publication: Option<SharedPayload>,
    installed: Option<SharedPayload>,
    replacement: Option<PendingSnapshot>,
    ready: bool,
    published_revision: u64,
    _memory: Arc<MemoryReservation>,
}

struct PendingSnapshot {
    publication: SharedPayload,
    revision: u64,
    decoded: usize,
}

/// The private type and broker-issued attachment jointly bind an immutable
/// verified closure to one authorized publication/view allocation.
struct PublishedClosure {
    revision: u64,
    resources: BTreeMap<ResourceDigest, PublishedResource>,
    _memory: Arc<ViewMemoryReservation>,
}
#[derive(Clone)]
struct PublishedResource {
    offer: ResourceOffer,
    bytes: Arc<[u8]>,
    _memory: Arc<ViewMemoryReservation>,
}

/// Cooperating-source fixture with mandatory core admission and actual native
/// callbacks. Trusted caller supplies verified authorization, never client JSON.
/// Public observer projections cannot serve as the native source authority.
pub struct GtkCommandHost {
    pipe: NativePipe,
    capture: NativeCapture,
    source: ViewReceiver,
    sessions: SessionBroker,
    ledger: CommandLedger,
    composition: composition::Composition,
    views: BTreeMap<Id, View>,
    native_lease: Option<(ControlLease, u64)>,
    broker_epoch: Id,
    source_incarnations: BTreeSet<Id>,
    public_revision: u64,
    source_offset: u64,
    source_alive: bool,
    identities: SourceIdentities,
    _identity_memory: MemoryReservation,
    last_canvas: Option<pb::Rect>,
    last_source_signal: Option<i32>,
    _source_memory: MemoryReservation,
    _ledger_memory: MemoryReservation,
}
impl GtkCommandHost {
    pub fn spawn(
        command: ProcessCommand,
        artifact_directory: impl AsRef<Path>,
    ) -> Result<Self, GtkHostError> {
        // Conservative aggregate reservations cover the separately bounded
        // core receiver/broker pools; no allocation is hidden by another pool.
        let budget = MemoryBudget::new(256 << 20);
        let source_memory = budget.reserve(SOURCE_MEMORY)?;
        let ledger_memory = budget.reserve(4 << 20)?;
        // Two sparse live maps plus issued/retired identity retention.
        let identity_memory = budget.reserve(4 << 20)?;
        if artifact_directory.as_ref().exists()
            && std::fs::read_dir(artifact_directory.as_ref())?
                .next()
                .is_some()
        {
            return Err(GtkHostError::Unsupported);
        }
        let (mut pipe, capture) = NativePipe::spawn(
            command,
            artifact_directory.as_ref().to_path_buf(),
            budget,
            1,
        )?;
        let mut source = ViewReceiver::new(
            capture.artifact.context(),
            ReceiverLimits {
                view_memory_bytes: SOURCE_MEMORY,
                ..ReceiverLimits::default()
            },
            ReceiverConfig {
                single_glyph_outlines: true,
                ..ReceiverConfig::default()
            },
        )
        .map_err(CaptureError::from)?;
        install_capture(&capture, &mut source, &pipe)?;
        pipe.acknowledge_capture(&capture)?;
        let context = capture.artifact.context();
        let mut config = SessionConfig {
            max_clients: MAX_CLIENTS,
            receiver_limits: ReceiverLimits {
                view_memory_bytes: PUBLIC_VIEW_MEMORY,
                envelope_bytes: 128 << 10,
                chunk_bytes: 32 << 10,
                ..ReceiverLimits::default()
            },
            ..SessionConfig::default()
        };
        config.supported_profiles.insert("semantics.only.v1".into());
        config
            .supported_profiles
            .insert("glyph.outline-single.v1".into());
        if capture.artifact.manifest()["gtk_version"] == serde_json::json!([4, 22, 4]) {
            config
                .supported_profiles
                .insert("text.clock.gtk-editable4224.v1".into());
        }
        let sessions = SessionBroker::with_memory_budget(
            context.scope.session_id,
            context.scope.epoch,
            config,
            pipe.budget.clone(),
        )?;
        let ledger = CommandLedger::new(
            context.scope.epoch,
            LedgerLimits {
                max_clients: MAX_CLIENTS,
                max_commands: 1024,
                max_retained_command_bytes: 1 << 20,
                max_command_bytes: 32 << 10,
                max_outcomes: 128,
                ..LedgerLimits::default()
            },
        )?;
        let composition = composition::Composition::new(&pipe.budget)?;
        let mut host = Self {
            pipe,
            capture,
            source,
            sessions,
            ledger,
            composition,
            views: BTreeMap::new(),
            native_lease: None,
            broker_epoch: context.scope.epoch,
            source_incarnations: [context.scope.epoch].into_iter().collect(),
            public_revision: 0,
            source_offset: 0,
            source_alive: true,
            identities: SourceIdentities::new(),
            _identity_memory: identity_memory,
            last_canvas: None,
            last_source_signal: None,
            _source_memory: source_memory,
            _ledger_memory: ledger_memory,
        };
        host.publish_all()?;
        Ok(host)
    }
    /// Trusted source supervisor operation; never decoded from peer commands.
    /// The broker epoch, negotiated views and common ledger survive replacement.
    pub fn restart_source(
        &mut self,
        command: ProcessCommand,
        directory: impl AsRef<Path>,
        now: u64,
    ) -> Result<(), GtkHostError> {
        if self.identities.generation() >= 5 {
            return Err(GtkHostError::Unsupported);
        }
        if directory.as_ref().exists() && std::fs::read_dir(directory.as_ref())?.next().is_some() {
            return Err(GtkHostError::Unsupported);
        }
        std::fs::create_dir_all(directory.as_ref())?;
        let replacement_directory = directory.as_ref().canonicalize()?;
        let old_directory = self.pipe.directory.canonicalize()?;
        if replacement_directory.starts_with(&old_directory)
            || old_directory.starts_with(&replacement_directory)
        {
            return Err(GtkHostError::Unsupported);
        }
        self.mark_source_unavailable(now)?;
        self.identities.reset()?;
        let (mut pipe, capture) = NativePipe::spawn(
            command,
            replacement_directory,
            self.pipe.budget.clone(),
            self.identities.generation(),
        )?;
        if self.capture.artifact.manifest()["gtk_version"] == serde_json::json!([4, 22, 4])
            && capture.artifact.manifest()["gtk_version"] != serde_json::json!([4, 22, 4])
        {
            // A negotiated backend clock promise survives this broker epoch;
            // replacement cannot silently change it beneath existing views.
            return Err(GtkHostError::Unsupported);
        }
        if self
            .source_incarnations
            .contains(&capture.artifact.context().scope.epoch)
        {
            return Err(GtkHostError::Unsupported);
        }
        let mut source = ViewReceiver::new(
            capture.artifact.context(),
            ReceiverLimits {
                view_memory_bytes: SOURCE_MEMORY,
                ..ReceiverLimits::default()
            },
            ReceiverConfig {
                single_glyph_outlines: true,
                ..ReceiverConfig::default()
            },
        )
        .map_err(CaptureError::from)?;
        install_capture(&capture, &mut source, &pipe)?;
        pipe.acknowledge_capture(&capture)?;
        self.pipe.purge_artifacts()?;
        self.source_incarnations
            .insert(capture.artifact.context().scope.epoch);
        self.source_offset = self.public_revision;
        self.pipe = pipe;
        self.composition = composition::Composition::new(&self.pipe.budget)?;
        self.capture = capture;
        self.source = source;
        self.source_alive = true;
        if let Err(error) = self.publish_all() {
            self.mark_source_unavailable(now)?;
            return Err(error);
        }
        Ok(())
    }
    /// Supervisor tick detects source exit even without a peer command.
    pub fn poll_source(&mut self, now: u64) -> Result<bool, GtkHostError> {
        if self.source_alive && self.pipe.child.try_wait()?.is_some() {
            self.mark_source_unavailable(now)?;
            return Ok(true);
        }
        Ok(false)
    }
    fn mark_source_unavailable(&mut self, now: u64) -> Result<(), GtkHostError> {
        if !self.source_alive {
            return Ok(());
        }
        self.source_alive = false;
        self.composition.cancel();
        self.pipe.fence.invalidate();
        self.pipe.failed = true;
        self.sessions.revoke_control(now);
        self.native_lease = None;
        let _ = self.pipe.socket.shutdown(std::net::Shutdown::Both);
        let _ = self.pipe.child.kill();
        use std::os::unix::process::ExitStatusExt;
        self.last_source_signal = self
            .pipe
            .child
            .wait()
            .ok()
            .and_then(|status| status.signal());
        // This semantic-only adapter never owns physical held input or IME.
        // The disposable process has exited; no native transient owner survives.
        while self.sessions.take_cleanup().is_some() {}
        let canvas = self.last_canvas.ok_or(GtkHostError::Unsupported)?;
        self.source.close();
        self.public_revision = self
            .public_revision
            .checked_add(1)
            .ok_or(GtkHostError::Unsupported)?;
        let snapshot = self.identities.unavailable(self.public_revision, canvas)?;
        let _working = self.pipe.budget.reserve(PROJECT_MEMORY)?;
        publish_snapshot(snapshot, 4096, None, &mut self.sessions, &mut self.views)
    }
    #[cfg(feature = "gtk-fixture-hooks")]
    pub fn native_pid(&self) -> u32 {
        self.pipe.child.id()
    }
    #[cfg(feature = "gtk-fixture-hooks")]
    pub fn source_generation(&self) -> u64 {
        self.identities.generation()
    }
    pub fn publication_revision(&self) -> u64 {
        self.public_revision
    }
    fn supports_text_composition(&self) -> bool {
        self.source_alive
            && self.capture.artifact.manifest()["surfaces"].is_array()
            && self.capture.artifact.manifest()["source_boundary_facts"].is_object()
    }
    #[cfg(feature = "gtk-fixture-hooks")]
    pub fn last_source_signal(&self) -> Option<i32> {
        self.last_source_signal
    }
    #[cfg(feature = "gtk-fixture-hooks")]
    pub fn source_memory_used(&self) -> usize {
        self.source.memory_used()
    }

    pub fn diagnostics(&self) -> NativeDiagnostics {
        self.pipe.diagnostics
    }
    /// Conservative host reservations, including its independently bounded core pools.
    pub fn memory_budget(&self) -> MemoryBudget {
        self.pipe.budget.clone()
    }
    /// Exact pending authorized bytes; callers cannot mutate the publication.
    pub fn pending_publication(
        &self,
        client: &AuthenticatedClient,
    ) -> Result<Option<&[u8]>, GtkHostError> {
        Ok(self.view(client)?.publication.as_deref())
    }
    pub fn epoch(&self) -> Id {
        self.broker_epoch
    }
    #[cfg(feature = "gtk-fixture-hooks")]
    pub fn source_capture_for_diagnostics(&self) -> &serde_json::Value {
        self.capture.artifact.manifest()
    }
    #[cfg(feature = "gtk-fixture-hooks")]
    pub fn source_artifact_for_diagnostics(&self) -> &Path {
        &self.capture.artifact.path
    }
    #[cfg(feature = "gtk-fixture-hooks")]
    pub fn trusted_fixture_mutation(
        &mut self,
        operation: FixtureMutation,
    ) -> Result<(), GtkHostError> {
        #[derive(Serialize)]
        struct Fixture {
            request_id: u64,
            kind: &'static str,
            operation: &'static str,
        }
        let reply = self.pipe.request(&Fixture {
            request_id: self.pipe.next_request,
            kind: "fixture",
            operation: operation.name(),
        })?;
        if reply.status == "fixture-capture" {
            let next = self.pipe.capture(reply)?;
            install_capture(&next, &mut self.source, &self.pipe)?;
            self.capture = next;
            self.publish_all()?;
            self.pipe.acknowledge_capture(&self.capture)?;
        } else if reply.status != "fixture" {
            return Err(GtkHostError::NativePipe);
        }
        Ok(())
    }
    pub fn negotiate(
        &mut self,
        authorization: VerifiedAuthorization,
        hello: ClientHello,
        now: u64,
    ) -> Result<NegotiatedSession, GtkHostError> {
        if self.views.contains_key(&hello.client_instance_id) {
            return Err(GtkHostError::Unsupported);
        }
        // The private receiver has an independent 8MiB allowance. Every core
        // session allocation carries its actual shared-parent reservation.
        let memory = Arc::new(self.pipe.budget.reserve(CLIENT_MEMORY)?);
        let negotiated = self.sessions.negotiate(Some(authorization), hello, now)?;
        let consumer = consumer_for_view(&mut self.sessions, &negotiated, now)?;
        self.views.insert(
            negotiated.client.client_instance_id,
            View {
                negotiated: negotiated.clone(),
                consumer,
                publication: None,
                installed: None,
                replacement: None,
                ready: true,
                published_revision: 0,
                _memory: memory,
            },
        );
        if let Err(error) = self.publish_all() {
            self.views.remove(&negotiated.client.client_instance_id);
            let _ = self
                .sessions
                .invalidate_authorization(&negotiated.client, now);
            return Err(error);
        }
        self.view(&negotiated.client)?;
        Ok(negotiated)
    }
    fn view(&self, client: &AuthenticatedClient) -> Result<&View, GtkHostError> {
        let view = self
            .views
            .get(&client.client_instance_id)
            .ok_or(SessionError::Unauthenticated)?;
        self.sessions
            .authorize_projection(client, view.negotiated.view_id)?;
        Ok(view)
    }
    pub fn state(&self, client: &AuthenticatedClient) -> Result<&State, GtkHostError> {
        Ok(self
            .view(client)?
            .consumer
            .state()
            .map_err(CaptureError::from)?)
    }
    pub fn presented_revision(
        &self,
        client: &AuthenticatedClient,
    ) -> Result<Option<u64>, GtkHostError> {
        let view = self.view(client)?;
        Ok(self
            .sessions
            .presented_revision(client, view.negotiated.view_id)?)
    }
    /// Actual local semantic client consumes exact authorized bytes. This does
    /// not mark Presented and cannot authorize raw input, IME or resize.
    pub fn consume_semantics(&mut self, client: &AuthenticatedClient) -> Result<u64, GtkHostError> {
        self.view(client)?;
        let view = self.views.get_mut(&client.client_instance_id).unwrap();
        if !view.negotiated.profiles.contains("semantics.only.v1") {
            return Err(GtkHostError::Unsupported);
        }
        let bytes = view.publication.take().ok_or(GtkHostError::Unsupported)?;
        let revision = match view
            .consumer
            .receive_envelope((self.public_revision as u128).to_be_bytes(), &bytes)
            .map_err(CaptureError::from)?
        {
            ReceiveOutcome::Applied { revision } | ReceiveOutcome::Duplicate { revision } => {
                revision
            }
            ReceiveOutcome::NeedResources { .. } => return Err(GtkHostError::Unsupported),
        };
        if !view
            .consumer
            .state()
            .map_err(CaptureError::from)?
            .resource_refs()
            .is_empty()
        {
            return Err(GtkHostError::Unsupported);
        }
        self.sessions
            .acknowledge_applied(client, view.negotiated.view_id, revision, true)?;
        self.sessions
            .acknowledge_semantic_ready(client, view.negotiated.view_id, revision)?;
        view.ready = true;
        install_replacement(&mut self.sessions, view)?;
        Ok(revision)
    }
    #[cfg(any(test, feature = "transport"))]
    fn acknowledge_applied(
        &mut self,
        client: &AuthenticatedClient,
        revision: u64,
        witness: &SharedPayload,
    ) -> Result<(), GtkHostError> {
        let view = self.view(client)?;
        let installed = view.installed.as_ref().ok_or(GtkHostError::Unsupported)?;
        let same_attachment = installed
            .attachment()
            .zip(witness.attachment())
            .is_some_and(|(a, b)| a.ptr_eq(b));
        if revision != view.published_revision || installed != witness || !same_attachment {
            return Err(SessionError::WrongRevision.into());
        }
        self.sessions
            .acknowledge_applied(client, view.negotiated.view_id, revision, true)?;
        Ok(())
    }
    #[cfg(any(test, feature = "transport"))]
    fn acknowledge_presented(
        &mut self,
        client: &AuthenticatedClient,
        revision: u64,
    ) -> Result<(), GtkHostError> {
        let view_id = self.view(client)?.negotiated.view_id;
        self.sessions
            .authorize_presented(client, view_id, revision)?;
        let view = self.views.get_mut(&client.client_instance_id).unwrap();
        if view.consumer.state().map_err(CaptureError::from)?.records.values().any(|record| {
            matches!(record, Record::Surface(surface) if surface.mapped && surface.visual_coverage != rui_protocol_core::model::VisualCoverage::Complete)
        }) {
            return Err(GtkHostError::Unsupported);
        }
        view.consumer
            .mark_presented(revision)
            .map_err(CaptureError::from)?;
        self.sessions
            .acknowledge_presented(client, view_id, revision)?;
        view.ready = true;
        install_replacement(&mut self.sessions, view)
    }
    #[cfg(any(test, feature = "transport"))]
    fn publication_resource(
        &self,
        client: &AuthenticatedClient,
        witness: &SharedPayload,
        digest: ResourceDigest,
    ) -> Result<PublishedResource, GtkHostError> {
        let view = self.view(client)?;
        let attachment = witness.attachment().ok_or(GtkHostError::Unsupported)?;
        self.sessions
            .authorize_attachment(client, view.negotiated.view_id, attachment)?;
        attachment
            .get::<PublishedClosure>()
            .and_then(|closure| closure.resources.get(&digest))
            .cloned()
            .ok_or(GtkHostError::Unsupported)
    }
    pub fn acquire_control(
        &mut self,
        client: &AuthenticatedClient,
        request: Id,
        now: u64,
    ) -> Result<ControlLease, GtkHostError> {
        self.acquire_control_guarded(client, request, now, || true)
    }
    fn acquire_control_guarded(
        &mut self,
        client: &AuthenticatedClient,
        request: Id,
        now: u64,
        authorized: impl FnOnce() -> bool,
    ) -> Result<ControlLease, GtkHostError> {
        if self.pipe.failed || !self.source_alive {
            return Err(GtkHostError::NativePipe);
        }
        let view = self.view(client)?.negotiated.view_id;
        let lease = self.sessions.acquire_control(client, view, request, now)?;
        if self.native_lease.as_ref().is_some_and(|(native, _)| {
            native.lease_id == lease.lease_id && native.generation == lease.generation
        }) {
            // An exact grant retry never extends the native deadline.
            if self.pipe.fence.current() != lease.generation {
                return Err(SessionError::StaleControl.into());
            }
            return Ok(lease);
        }
        if let Err(error) = self.pipe.fence.grant_if(lease.generation, authorized) {
            self.sessions.revoke_control(now);
            self.drain_native_cleanup();
            return Err(error.into());
        }
        self.install_native_control(lease, now)
    }
    #[cfg(feature = "transport")]
    fn renew_control_guarded(
        &mut self,
        client: &AuthenticatedClient,
        request: Id,
        lease_id: Id,
        generation: u64,
        now: u64,
        authorized: impl FnOnce() -> bool,
    ) -> Result<ControlLease, GtkHostError> {
        let view = self.view(client)?.negotiated.view_id;
        let lease = self
            .sessions
            .renew_control(client, view, request, lease_id, generation, now)?;
        if self.native_lease.as_ref().is_none_or(|(native, _)| {
            native.client != *client
                || native.lease_id != lease_id
                || native.generation != generation
        }) || self.pipe.fence.current() != generation
            || !authorized()
        {
            self.sessions.revoke_control(now);
            self.drain_native_cleanup();
            return Err(SessionError::StaleControl.into());
        }
        // An exact old renew retry cannot regress a later native deadline.
        if self
            .native_lease
            .as_ref()
            .is_some_and(|(native, _)| lease.expires_at_ms <= native.expires_at_ms)
        {
            return Ok(lease);
        }
        self.install_native_control(lease, now)
    }
    fn install_native_control(
        &mut self,
        lease: ControlLease,
        now: u64,
    ) -> Result<ControlLease, GtkHostError> {
        if self.native_lease.as_ref().is_none_or(|(old, _)| {
            old.lease_id != lease.lease_id || old.generation != lease.generation
        }) {
            self.composition.cancel();
        }
        let deadline_ns = monotonic_ns()?
            .checked_add(
                lease
                    .expires_at_ms
                    .saturating_sub(now)
                    .checked_mul(1_000_000)
                    .ok_or(GtkHostError::Unsupported)?,
            )
            .ok_or(GtkHostError::Unsupported)?;
        #[derive(Serialize)]
        struct Control {
            request_id: u64,
            kind: &'static str,
            lease_id: String,
            generation: u64,
            deadline_ns: u64,
        }
        let reply = self.pipe.request(&Control {
            request_id: self.pipe.next_request,
            kind: "control",
            lease_id: lease
                .lease_id
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            generation: lease.generation,
            deadline_ns,
        });
        if !reply.is_ok_and(|reply| reply.status == "control") {
            self.mark_source_unavailable(now)?;
            return Err(GtkHostError::NativePipe);
        }
        self.native_lease = Some((lease.clone(), deadline_ns));
        Ok(lease)
    }
    pub fn release_control(
        &mut self,
        client: &AuthenticatedClient,
        request: Id,
        lease: Id,
        generation: u64,
        now: u64,
    ) -> Result<(), GtkHostError> {
        self.view(client)?;
        self.sessions
            .release_control(client, request, lease, generation, now)?;
        if self.native_lease.as_ref().is_some_and(|(native, _)| {
            native.client == *client && native.lease_id == lease && native.generation == generation
        }) {
            self.pipe.fence.invalidate();
            self.composition.cancel();
            self.native_lease = None;
        }
        self.drain_native_cleanup();
        Ok(())
    }
    fn drain_native_cleanup(&mut self) {
        // Invalidate immediately. The source thread independently polls the
        // atomic fence and clears private preedit; no late commit can pass it.
        while let Some(cleanup) = self.sessions.take_cleanup() {
            self.composition.cancel();
            if self.native_lease.as_ref().is_some_and(|(native, _)| {
                native.lease_id == cleanup.lease.lease_id
                    && native.generation == cleanup.lease.generation
            }) {
                self.pipe.fence.invalidate();
                self.native_lease = None;
            }
        }
    }
    #[cfg(feature = "gtk-fixture-hooks")]
    pub fn invalidate_native_fence_when_queued(
        &self,
    ) -> Result<std::thread::JoinHandle<Result<serde_json::Value, GtkHostError>>, GtkHostError>
    {
        let memory = self.pipe.budget.reserve(256 << 10)?;
        let fence = self.pipe.fence.clone();
        let receipt_path = self.pipe.directory.join("native-fence-queued.json");
        let request_id = self.pipe.next_request;
        let source_generation = self.pipe.source_generation;
        let generation = self
            .native_lease
            .as_ref()
            .ok_or(GtkHostError::NativePipe)?
            .0
            .generation;
        Ok(std::thread::Builder::new()
            .stack_size(128 << 10)
            .spawn(move || {
                let _memory = memory;
                let deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    match bounded_read(&receipt_path, 1024) {
                        Ok(bytes) => {
                            let receipt: serde_json::Value = match serde_json::from_slice(&bytes) {
                                Ok(receipt) => receipt,
                                // The bounded source file can be observed while
                                // its single write is still completing.
                                Err(error) if error.is_eof() && Instant::now() < deadline => {
                                    std::thread::sleep(Duration::from_millis(1));
                                    continue;
                                }
                                Err(_) => return Err(GtkHostError::NativePipe),
                            };
                            if receipt["private_request_id"] != request_id
                                || receipt["source_generation"] != source_generation
                                || receipt["control_generation"] != generation
                                || receipt["fence_generation"] != generation
                                || receipt["native_dispatches"] != 0
                                || receipt["observed_monotonic_ns"].as_u64().is_none()
                                || fence.current() != generation
                            {
                                return Err(GtkHostError::NativePipe);
                            }
                            fence.invalidate();
                            return Ok(receipt);
                        }
                        Err(CaptureError::Io(error))
                            if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(_) => return Err(GtkHostError::NativePipe),
                    }
                    if Instant::now() >= deadline {
                        return Err(GtkHostError::NativePipe);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            })?)
    }
    pub fn revoke_authorization(
        &mut self,
        client: &AuthenticatedClient,
        now: u64,
    ) -> Result<(), GtkHostError> {
        if self
            .native_lease
            .as_ref()
            .is_some_and(|(lease, _)| lease.client == *client)
        {
            self.pipe.fence.invalidate();
            self.native_lease = None;
        }
        self.sessions.invalidate_authorization(client, now)?;
        self.drain_native_cleanup();
        if let Some(view) = self.views.remove(&client.client_instance_id) {
            drop(view);
        }
        Ok(())
    }
    pub fn disconnect(
        &mut self,
        client: &AuthenticatedClient,
        now: u64,
    ) -> Result<(), GtkHostError> {
        if self
            .native_lease
            .as_ref()
            .is_some_and(|(lease, _)| lease.client == *client)
        {
            self.pipe.fence.invalidate();
            self.native_lease = None;
        }
        self.sessions.disconnect(client, now)?;
        // This adapter disposes its private consumer and exact publication
        // keeper on disconnect. Retire the public view at the same boundary;
        // reconnect must use a fresh snapshot scope rather than claim resume
        // from a keeper this owner no longer retains. The broker retains the
        // principal binding and the common command ledger remains unchanged.
        self.sessions.invalidate_authorization(client, now)?;
        self.drain_native_cleanup();
        if let Some(view) = self.views.remove(&client.client_instance_id) {
            drop(view);
        }
        Ok(())
    }
    fn publish_all(&mut self) -> Result<(), GtkHostError> {
        if !self.source_alive {
            let snapshot = self.identities.unavailable(
                self.public_revision,
                self.last_canvas.ok_or(GtkHostError::Unsupported)?,
            )?;
            let _working = self.pipe.budget.reserve(PROJECT_MEMORY)?;
            return publish_snapshot(snapshot, 4096, None, &mut self.sessions, &mut self.views);
        }
        self.last_canvas = self
            .source
            .state()
            .map_err(CaptureError::from)?
            .records
            .values()
            .find_map(|r| {
                if let Record::Session(s) = r {
                    Some(pb::Rect {
                        x: s.canvas.x,
                        y: s.canvas.y,
                        width: s.canvas.width,
                        height: s.canvas.height,
                    })
                } else {
                    None
                }
            });
        publish(
            &self.capture,
            &self.source,
            &self.pipe,
            &mut self.sessions,
            &mut self.views,
            Projection {
                identities: &mut self.identities,
                offset: self.source_offset,
                revision: &mut self.public_revision,
            },
        )?;
        self.synchronize_composition(0)
    }
    fn synchronize_composition(&mut self, now: u64) -> Result<(), GtkHostError> {
        if self
            .composition
            .synchronize(composition::context(&self.source, &self.identities)?)
        {
            if let Some((lease, _)) = &self.native_lease {
                // Clear the matching broker obligation when its control scope
                // remains ready. After readiness loss/revocation/expiry the
                // conservatively retained obligation drains through mandatory
                // cleanup; native transient state is already canceled here.
                let _ = self.sessions.set_composition(lease, None, now);
            }
        }
        Ok(())
    }
    pub fn refresh_source(&mut self) -> Result<(), GtkHostError> {
        self.capture_source(false).map(|_| ())
    }
    /// Publish local native changes without creating revisions for idle polls.
    /// This uses the same fresh capture/verification path as admitted commands.
    pub fn refresh_if_changed(&mut self) -> Result<bool, GtkHostError> {
        self.capture_source(true)
    }
    fn capture_source(&mut self, only_if_changed: bool) -> Result<bool, GtkHostError> {
        let result = (|| {
            let reply = self.pipe.request(&CaptureRequest {
                request_id: self.pipe.next_request,
                kind: if only_if_changed {
                    "capture-if-changed"
                } else {
                    "capture"
                },
            })?;
            if only_if_changed && reply.status == "unchanged" {
                return Ok(false);
            }
            if reply.status != "capture" {
                return Err(GtkHostError::NativePipe);
            }
            let capture = self.pipe.capture(reply)?;
            install_capture(&capture, &mut self.source, &self.pipe)?;
            self.capture = capture;
            self.publish_all()?;
            self.pipe.acknowledge_capture(&self.capture)?;
            Ok(true)
        })();
        if result.is_err() {
            self.mark_source_unavailable(0)?;
        }
        result
    }
    /// Exact peer bytes enter the one shared ledger. Native write is private to
    /// its admitted callback; retries return retained outcomes without a pipe call.
    pub fn submit(
        &mut self,
        client: &AuthenticatedClient,
        family: CommandFamily,
        bytes: &[u8],
        now: u64,
    ) -> Result<CommandResult, GtkHostError> {
        let _working = self.pipe.budget.reserve(PROJECT_MEMORY)?;
        self.poll_source(now)?;
        let view_id = self.view(client)?.negotiated.view_id;
        let composition_allowed = !self
            .view(client)?
            .negotiated
            .profiles
            .contains("semantics.only.v1")
            && self.supports_text_composition();
        if self.source_alive {
            self.synchronize_composition(now)?;
        }
        let authority = if self.source_alive {
            Some(public_authority(
                &self.source,
                view_id,
                &self.identities,
                self.public_revision,
            )?)
        } else {
            None
        };
        let pipe = &mut self.pipe;
        let capture = &mut self.capture;
        let source = &mut self.source;
        let views = &mut self.views;
        let identities = &mut self.identities;
        let public_revision = &mut self.public_revision;
        let source_offset = self.source_offset;
        let native_deadline = self.native_lease.as_ref().map(|(_, deadline)| *deadline);
        let native_lease = self.native_lease.as_ref().map(|(lease, _)| lease.clone());
        let mut attempted_native_dispatch = false;
        let result = {
            let composition = std::cell::RefCell::new(&mut self.composition);
            self.ledger.submit_controlled_with_session(
                &mut self.sessions,
                client,
                family,
                bytes,
                now,
                |command| {
                    if let Command::TextComposition(command) = command {
                        if !composition_allowed {
                            return Err(Rejection::UnsupportedAction);
                        }
                        return composition.borrow().validate(client, command);
                    }
                    let Command::Semantic(command) = command else {
                        return Err(Rejection::UnsupportedAction);
                    };
                    let authority = authority.as_ref().ok_or(Rejection::MissingTarget)?;
                    authority.validate(command)?;
                    let owns_model = authority
                        .targets
                        .get(&command.target_id)
                        .is_some_and(|target| target.model_version.is_some());
                    if matches!(command.action_id.as_str(), "Activate" | "ScrollTo")
                        && owns_model
                        && command.preconditions.model_version.is_none()
                    {
                        return Err(Rejection::InvalidArguments);
                    }
                    if matches!(&command.arguments, Arguments::ReplaceText { text, .. }
                    if text.len() > 16 << 10 || text.contains('\0'))
                    {
                        return Err(Rejection::InvalidArguments);
                    }
                    Ok(())
                },
                |command, sessions| {
                    // Reserve pending metadata/body scratch before native execution,
                    // alongside the already reserved command/projection scratch.
                    let pending_memory = pipe
                        .budget
                        .reserve(ARTIFACT_MEMORY)
                        .map_err(|_| DispatchFailure::Failed)?;
                    let _capture_working = pipe
                        .budget
                        .reserve(CAPTURE_WORKING_MEMORY)
                        .map_err(|_| DispatchFailure::Failed)?;
                    let deadline = native_deadline.ok_or(DispatchFailure::Failed)?;
                    let request_id = pipe.next_request;
                    let request = match command {
                        Command::Semantic(command) => NativeRequest::Semantic(
                            native_command(
                                &capture.artifact,
                                source,
                                authority.as_ref().ok_or(DispatchFailure::Failed)?,
                                command,
                                request_id,
                                deadline,
                                identities,
                            )
                            .map_err(|_| DispatchFailure::Failed)?,
                        ),
                        Command::TextComposition(command) => NativeRequest::Composition(
                            native_composition(
                                &capture.artifact,
                                composition
                                    .borrow()
                                    .context()
                                    .ok_or(DispatchFailure::Failed)?,
                                command,
                                request_id,
                                deadline,
                            )
                            .map_err(|_| DispatchFailure::Failed)?,
                        ),
                        _ => return Err(DispatchFailure::Failed),
                    };
                    attempted_native_dispatch = true;
                    let reply = pipe
                        .request(&request)
                        .map_err(|_| DispatchFailure::OutcomeUnknown)?;
                    if reply.status == "rejected" {
                        if matches!(command, Command::TextComposition(_)) {
                            composition.borrow_mut().cancel();
                            if let Some(lease) = &native_lease {
                                let _ = sessions.set_composition(lease, None, now);
                            }
                        }
                        return Err(DispatchFailure::Failed);
                    }
                    if let Command::TextComposition(command) = command {
                        let commit =
                            matches!(command.operation, CompositionOperation::CommitText { .. });
                        if (!commit && reply.status != "dispatched")
                            || (commit && reply.status != "applied")
                        {
                            return Err(DispatchFailure::OutcomeUnknown);
                        }
                        composition.borrow_mut().acknowledge(client, command)?;
                        let active = if matches!(
                            command.operation,
                            CompositionOperation::Begin(_)
                                | CompositionOperation::UpdatePreedit { .. }
                        ) {
                            Some(command.text_session_id)
                        } else {
                            None
                        };
                        sessions
                            .set_composition(
                                native_lease
                                    .as_ref()
                                    .ok_or(DispatchFailure::OutcomeUnknown)?,
                                active,
                                now,
                            )
                            .map_err(|_| DispatchFailure::OutcomeUnknown)?;
                        if !commit {
                            return Ok(DispatchOutcome::Dispatched);
                        }
                    }
                    if reply.status != "applied" {
                        return Err(DispatchFailure::OutcomeUnknown);
                    }
                    let next = pipe
                        .capture_reserved(reply, pending_memory)
                        .map_err(|_| DispatchFailure::OutcomeUnknown)?;
                    let manifest = next.artifact.manifest();
                    let current = source
                        .state()
                        .map_err(|_| DispatchFailure::OutcomeUnknown)?
                        .revision;
                    if next.artifact.context() != capture.artifact.context()
                        || manifest["command_cause"]["private_request_id"].as_u64()
                            != Some(request_id)
                        || manifest["command_cause"]["source_generation"].as_u64()
                            != Some(identities.generation())
                        || manifest["command_cause"]["authority_gate"].as_str()
                            != Some("core-ledger-admitted")
                        || manifest["revision"]
                            .as_u64()
                            .is_none_or(|revision| revision <= current)
                    {
                        return Err(DispatchFailure::OutcomeUnknown);
                    }
                    next.artifact
                        .apply_trusted_fixture(source)
                        .map_err(|_| DispatchFailure::OutcomeUnknown)?;
                    verify_capture(&next, source).map_err(|_| DispatchFailure::OutcomeUnknown)?;
                    *capture = next;
                    publish(
                        capture,
                        source,
                        pipe,
                        sessions,
                        views,
                        Projection {
                            identities: &mut *identities,
                            offset: source_offset,
                            revision: &mut *public_revision,
                        },
                    )
                    .map_err(|_| DispatchFailure::OutcomeUnknown)?;
                    pipe.acknowledge_capture(capture)
                        .map_err(|_| DispatchFailure::OutcomeUnknown)?;
                    let revision = *public_revision;
                    let token = if let Command::TextComposition(command) = command {
                        // The whole-view cause/nonce/source-generation audit
                        // above established this fresh capture. Name the
                        // actual editing surface's witness so a client can
                        // bind completion to its installed causal surface.
                        let state = source
                            .state()
                            .map_err(|_| DispatchFailure::OutcomeUnknown)?;
                        let target = identities
                            .private_id(command.target_id)
                            .map_err(|_| DispatchFailure::OutcomeUnknown)?;
                        let surface_id = semantic_surface(state, target)
                            .map_err(|_| DispatchFailure::OutcomeUnknown)?;
                        let Some(Record::Surface(surface)) = state.records.get(&surface_id) else {
                            return Err(DispatchFailure::OutcomeUnknown);
                        };
                        let [token] = surface.source_capture_tokens.as_slice() else {
                            return Err(DispatchFailure::OutcomeUnknown);
                        };
                        String::from_utf8(token.clone())
                            .map_err(|_| DispatchFailure::OutcomeUnknown)?
                    } else {
                        capture.artifact.manifest()["capture_token"]
                            .as_str()
                            .ok_or(DispatchFailure::OutcomeUnknown)?
                            .to_owned()
                    };
                    let published = views
                        .get(&client.client_instance_id)
                        .filter(|view| view.published_revision == revision)
                        .map(|_| revision);
                    let evidence = SourceEvidence::new(token, published)
                        .map_err(|_| DispatchFailure::OutcomeUnknown)?;
                    Ok(DispatchOutcome::Applied(evidence))
                },
            )?
        };
        if self.source_alive {
            self.synchronize_composition(now)?;
        }
        if attempted_native_dispatch
            && result.status == rui_protocol_core::commands::CommandStatus::OutcomeUnknown
        {
            self.mark_source_unavailable(now)?;
        }
        Ok(result)
    }
}

/// Explicit controlled-fixture source mutations for race/lifecycle tests. This
/// feature is never decoded from a peer command and is not a production action.
#[cfg(feature = "gtk-fixture-hooks")]
pub enum FixtureMutation {
    RaceEdit,
    RaceDestroy,
    RaceVisual,
    RaceModel,
    RaceSourceGeneration,
    RaceFocusCycle,
    DelayForFence,
    SelectionCycle,
    CompositionState,
    CompositionFocusCycle,
    CompositionRemove,
    CompositionEmpty,
    InterruptWholeCapture,
    DisableButton,
    EnableButton,
    DestroySurface,
    LoseAfterDispatch,
    StallAfterDispatch,
    DelayUntilExpired,
    UncorrelatedCapture,
    CrashAfterDispatch,
}
#[cfg(feature = "gtk-fixture-hooks")]
impl FixtureMutation {
    fn name(&self) -> &'static str {
        match self {
            Self::RaceEdit => "race-edit",
            Self::RaceDestroy => "race-destroy",
            Self::RaceVisual => "race-visual",
            Self::RaceModel => "race-model",
            Self::RaceSourceGeneration => "race-source-generation",
            Self::RaceFocusCycle => "race-focus-cycle",
            Self::DelayForFence => "delay-for-fence",
            Self::SelectionCycle => "selection-cycle",
            Self::CompositionState => "composition-state",
            Self::CompositionFocusCycle => "composition-focus-cycle",
            Self::CompositionRemove => "composition-remove",
            Self::CompositionEmpty => "composition-empty",
            Self::InterruptWholeCapture => "interrupt-whole-capture",
            Self::DisableButton => "disable-button",
            Self::EnableButton => "enable-button",
            Self::DestroySurface => "destroy-surface",
            Self::LoseAfterDispatch => "lose-after-dispatch",
            Self::StallAfterDispatch => "stall-after-dispatch",
            Self::DelayUntilExpired => "delay-until-expired",
            Self::UncorrelatedCapture => "uncorrelated-capture",
            Self::CrashAfterDispatch => "crash-after-dispatch",
        }
    }
}

fn consumer_for_view(
    sessions: &mut SessionBroker,
    negotiated: &NegotiatedSession,
    now: u64,
) -> Result<ViewReceiver, GtkHostError> {
    let context = ResourceContext {
        scope: ResourceScope {
            session_id: negotiated.session_id,
            epoch: negotiated.epoch,
            principal_id: negotiated.client.client_instance_id,
        },
        view_id: negotiated.view_id,
    };
    let mut limits = negotiated.limits.clone();
    limits.view_memory_bytes = limits.view_memory_bytes.min(CLIENT_MEMORY);
    match ViewReceiver::new(
        context,
        limits,
        ReceiverConfig {
            single_glyph_outlines: negotiated.profiles.contains("glyph.outline-single.v1"),
            ..ReceiverConfig::default()
        },
    ) {
        Ok(consumer) => Ok(consumer),
        Err(error) => {
            // A session registered just before this allocation must never stay
            // authorized without the consumer that owns its readiness/state.
            sessions.invalidate_authorization(&negotiated.client, now)?;
            Err(CaptureError::from(error).into())
        }
    }
}

fn install_capture(
    capture: &NativeCapture,
    source: &mut ViewReceiver,
    pipe: &NativePipe,
) -> Result<(), GtkHostError> {
    let _working = pipe.budget.reserve(CAPTURE_WORKING_MEMORY)?;
    capture.artifact.apply_trusted_fixture(source)?;
    verify_capture(capture, source)?;
    Ok(())
}

fn verify_capture(capture: &NativeCapture, source: &ViewReceiver) -> Result<(), GtkHostError> {
    capture.artifact.verify_vector_audit(source)?;
    let manifest = capture.artifact.manifest();
    let state = source.state().map_err(CaptureError::from)?;
    if manifest["revision"].as_u64() != Some(state.revision) {
        return Err(GtkHostError::Unsupported);
    }
    if manifest["capture_boundary"].as_str()
        == Some("source-thread-all-registered-surfaces-fresh-snapshot-barrier")
    {
        // The shared vector audit already validates exact whole-view surface
        // membership, nonce, topology, focus, metadata and source token binding.
        return Ok(());
    }
    let surface = manifest["surface_id"]
        .as_u64()
        .ok_or(GtkHostError::Unsupported)?;
    if manifest["destroyed"].as_bool() == Some(true) {
        if state.records.contains_key(&surface) {
            return Err(GtkHostError::Unsupported);
        }
    } else {
        let Some(Record::Surface(surface)) = state.records.get(&surface) else {
            return Err(GtkHostError::Unsupported);
        };
        let token = manifest["capture_token"]
            .as_str()
            .ok_or(GtkHostError::Unsupported)?;
        if surface.source_capture_tokens.len() != 1
            || surface.source_capture_tokens[0] != token.as_bytes()
        {
            return Err(GtkHostError::Unsupported);
        }
    }
    Ok(())
}

fn semantic_surface(state: &State, target: u64) -> Result<u64, GtkHostError> {
    for record in state.records.values() {
        let Record::Surface(surface) = record else {
            continue;
        };
        let Some(root) = surface.semantic_root else {
            continue;
        };
        let mut pending = vec![root.id];
        while let Some(id) = pending.pop() {
            if id == target {
                return Ok(surface.id);
            }
            let Some(Record::Semantic(node)) = state.records.get(&id) else {
                return Err(GtkHostError::Unsupported);
            };
            pending.extend(node.children.iter().map(|reference| reference.id));
        }
    }
    Err(GtkHostError::Unsupported)
}
fn surface_descends_from(state: &State, mut id: u64, ancestor: u64) -> bool {
    // The installed graph already guarantees acyclic typed surface ownership.
    loop {
        if id == ancestor {
            return true;
        }
        let Some(Record::Surface(surface)) = state.records.get(&id) else {
            return false;
        };
        let Some(parent) = surface.parent else {
            return false;
        };
        id = parent.id;
    }
}
fn authority(source: &ViewReceiver, view: Id) -> Result<SemanticAuthority, GtkHostError> {
    let state = source.state().map_err(CaptureError::from)?;
    let focus = state.records.values().find_map(|record| {
        if let Record::Session(session) = record {
            Some(session.focus_generation)
        } else {
            None
        }
    });
    let mut targets = BTreeMap::new();
    let mut item_models = BTreeMap::new();
    for record in state.records.values() {
        if let Record::Semantic(semantic) = record {
            if let Some(collection) = &semantic.collection {
                for item in &collection.items {
                    if item_models.insert(item.id, collection.version).is_some() {
                        return Err(GtkHostError::Unsupported);
                    }
                }
            }
        }
    }
    let allowed_actions: BTreeSet<_> = [
        "Activate",
        "ReplaceText",
        "SetSelection",
        "Focus",
        "ScrollTo",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for record in state.records.values() {
        let Record::Semantic(semantic) = record else {
            continue;
        };
        let owner = semantic_surface(state, semantic.id)?;
        let Some(Record::Surface(surface)) = state.records.get(&owner) else {
            return Err(GtkHostError::Unsupported);
        };
        if !surface.mapped
            || state.records.values().any(|record| {
                matches!(record,
            Record::Surface(modal) if modal.mapped && modal.modal_for.is_some_and(|reference|
                !surface_descends_from(state, owner, modal.id)
                    && surface_descends_from(state, owner, reference.id)))
            })
        {
            continue;
        }
        if semantic.sensitivity != Sensitivity::Public {
            return Err(GtkHostError::Unsupported);
        }
        let text = semantic
            .text
            .and_then(|reference| match &state.records[&reference.id] {
                Record::Text(text) => Some(text),
                _ => None,
            });
        let editable =
            text.is_some_and(|text| text.editable && text.status == TextStatus::Available);
        let actions = semantic
            .actions
            .iter()
            .filter(|action| {
                allowed_actions.contains(&action.id)
                    && (!matches!(action.id.as_str(), "ReplaceText" | "SetSelection") || editable)
                    && (action.id != "Activate"
                        || matches!(semantic.role, SemanticRole::Button | SemanticRole::ListItem))
            })
            .map(|action| (action.id.clone(), action.schema_version))
            .collect();
        targets.insert(
            semantic.id,
            TargetState {
                enabled: semantic.states.get(&StateKey::Disabled) != Some(&TriState::True),
                target_version: semantic.version,
                text_version: text.map(|text| text.text_version),
                model_version: semantic
                    .collection
                    .as_ref()
                    .map(|collection| collection.version)
                    .or_else(|| item_models.get(&semantic.id).copied()),
                focus_generation: focus,
                text_scalar_count: text
                    .and_then(|text| text.content.as_ref())
                    .map(|text| text.chars().count() as u64),
                value_schema: None,
                actions,
            },
        );
    }
    Ok(SemanticAuthority {
        view_id: view,
        current_view_revision: state.revision,
        allowed_actions,
        targets,
    })
}

fn public_authority(
    source: &ViewReceiver,
    view: Id,
    identities: &SourceIdentities,
    revision: u64,
) -> Result<SemanticAuthority, GtkHostError> {
    let raw = authority(source, view)?;
    let mut targets = BTreeMap::new();
    for (id, mut target) in raw.targets {
        target.focus_generation = Some(identities.focus_generation);
        targets.insert(identities.public_id(id)?, target);
    }
    Ok(SemanticAuthority {
        view_id: view,
        current_view_revision: revision,
        allowed_actions: raw.allowed_actions,
        targets,
    })
}

fn native_command<'a>(
    artifact: &CaptureArtifact,
    source: &ViewReceiver,
    authority: &SemanticAuthority,
    command: &'a SemanticCommand,
    request_id: u64,
    deadline_ns: u64,
    identities: &SourceIdentities,
) -> Result<NativeCommand<'a>, GtkHostError> {
    let target = authority
        .targets
        .get(&command.target_id)
        .ok_or(GtkHostError::Unsupported)?;
    let manifest = artifact.manifest();
    let private_target = identities.private_id(command.target_id)?;
    let guard = SourceGuard {
        source_generation: identities.generation(),
        incarnation: manifest["binding"]["epoch"]
            .as_str()
            .ok_or(GtkHostError::Unsupported)?
            .to_owned(),
        surface_id: semantic_surface(source.state().map_err(CaptureError::from)?, private_target)?,
        target_id: private_target,
        target_version: target.target_version,
        text_version: command.preconditions.text_version,
        model_version: command.preconditions.model_version,
        focus_generation: command
            .preconditions
            .focus_generation
            .map(|_| {
                manifest["session"]["focus_generation"]
                    .as_u64()
                    .ok_or(GtkHostError::Unsupported)
            })
            .transpose()?,
        lease_id: command
            .control_lease_id
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        control_generation: command.control_generation,
        deadline_ns,
        text_record_id: None,
        text_record_version: None,
        selection: None,
    };
    let arguments = match &command.arguments {
        Arguments::None => NativeArguments::None {},
        Arguments::ReplaceText { start, end, text } => NativeArguments::Text {
            start: *start,
            end: *end,
            text,
        },
        Arguments::Selection { anchor, focus } => NativeArguments::Selection {
            anchor: *anchor,
            focus: *focus,
        },
        Arguments::Scroll { x, y } => NativeArguments::Scroll { x: *x, y: *y },
        _ => return Err(GtkHostError::Unsupported),
    };
    Ok(NativeCommand {
        request_id,
        kind: "semantic",
        guard,
        action: &command.action_id,
        arguments,
    })
}

fn native_composition<'a>(
    artifact: &CaptureArtifact,
    context: &composition::Context,
    command: &'a TextCompositionCommand,
    request_id: u64,
    deadline_ns: u64,
) -> Result<NativeComposition<'a>, GtkHostError> {
    let (stage, range, text, selection) = match &command.operation {
        CompositionOperation::Begin(range) => ("begin", Some((range.start, range.end)), None, None),
        CompositionOperation::UpdatePreedit { text, selection } => (
            "update",
            None,
            Some(text.as_str()),
            Some((selection.start, selection.end)),
        ),
        CompositionOperation::CommitText { text } => ("commit", None, Some(text.as_str()), None),
        CompositionOperation::Cancel => ("cancel", None, None, None),
    };
    let hex = |id: Id| {
        id.iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    Ok(NativeComposition {
        request_id,
        kind: "composition",
        stage,
        range,
        text,
        selection,
        text_session_id: hex(command.text_session_id),
        guard: SourceGuard {
            source_generation: context.source_generation,
            incarnation: artifact.manifest()["binding"]["epoch"]
                .as_str()
                .ok_or(GtkHostError::Unsupported)?
                .to_owned(),
            surface_id: context.surface,
            target_id: context.private_target,
            target_version: context.target_version,
            text_version: Some(context.text_version),
            model_version: None,
            focus_generation: Some(context.native_focus_generation),
            lease_id: hex(command.header.control_lease_id),
            control_generation: command.header.control_generation,
            deadline_ns,
            text_record_id: Some(context.text_id),
            text_record_version: Some(context.text_record_version),
            selection: context.selection,
        },
    })
}

fn monotonic_ns() -> Result<u64, GtkHostError> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // Linux CLOCK_MONOTONIC is a shared kernel clock across these two local
    // processes. The pointer addresses a live initialized timespec.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    u64::try_from(value.tv_sec)
        .ok()
        .and_then(|seconds| seconds.checked_mul(1_000_000_000))
        .and_then(|seconds| {
            u64::try_from(value.tv_nsec)
                .ok()
                .and_then(|nanos| seconds.checked_add(nanos))
        })
        .ok_or(GtkHostError::Unsupported)
}

struct Projection<'a> {
    identities: &'a mut SourceIdentities,
    offset: u64,
    revision: &'a mut u64,
}
fn publish(
    capture: &NativeCapture,
    source: &ViewReceiver,
    pipe: &NativePipe,
    sessions: &mut SessionBroker,
    views: &mut BTreeMap<Id, View>,
    projection: Projection<'_>,
) -> Result<(), GtkHostError> {
    let Projection {
        identities,
        offset: source_offset,
        revision: public_revision,
    } = projection;
    if source.state().map_err(CaptureError::from)?.records.values().any(|record| matches!(record, Record::Semantic(node) if node.sensitivity != Sensitivity::Public)) {
        // This fixture has no generic redaction policy. Never send non-public
        // source semantics through the deliberately public projection.
        return Err(GtkHostError::Unsupported);
    }
    let _working = pipe.budget.reserve(PROJECT_MEMORY)?;
    let bytes = bounded_read(
        &capture.artifact.path.with_extension("bin"),
        MAX_ENVELOPE_BYTES,
    )?;
    let limits = Limits {
        max_envelope_bytes: MAX_ENVELOPE_BYTES as usize,
        max_snapshot_bytes: MAX_ENVELOPE_BYTES as usize,
        max_memory_bytes: MAX_MANIFEST_ALLOCATION,
        ..Limits::default()
    };
    let envelope = wire::decode_envelope(&bytes, &limits).map_err(|_| GtkHostError::Unsupported)?;
    let mut snapshot =
        wire::decode_snapshot(&envelope.payload, &limits).map_err(|_| GtkHostError::Unsupported)?;
    identities.project(&mut snapshot, source_offset)?;
    *public_revision = snapshot.revision;
    publish_snapshot(
        snapshot,
        source
            .state()
            .map_err(CaptureError::from)?
            .estimated_bytes(),
        Some(source),
        sessions,
        views,
    )
}

fn semantic_projection(snapshot: &mut pb::Snapshot) {
    snapshot
        .records
        .retain_mut(|record| match record.record.as_mut() {
            Some(pb::record::Record::Surface(surface)) => {
                surface.scene_root = None;
                surface.visual_coverage = 3;
                surface
                    .unsupported_features
                    .push("semantic-only-projection".into());
                true
            }
            Some(pb::record::Record::Semantic(semantic)) => {
                semantic.visual_bindings.clear();
                true
            }
            Some(pb::record::Record::Text(_)) => true,
            Some(pb::record::Record::Session(session)) => {
                session.cursor = None;
                true
            }
            _ => false,
        });
    snapshot.required_resources.clear();
    snapshot.required_features = vec![
        "semantics.core.v1".into(),
        "semantics.only.v1".into(),
        "content.no-raster.v1".into(),
    ];
}

fn publish_snapshot(
    snapshot: pb::Snapshot,
    decoded: usize,
    source: Option<&ViewReceiver>,
    sessions: &mut SessionBroker,
    views: &mut BTreeMap<Id, View>,
) -> Result<(), GtkHostError> {
    views.retain(|_, view| {
        if view.published_revision == snapshot.revision
            || view
                .replacement
                .as_ref()
                .is_some_and(|pending| pending.revision == snapshot.revision)
        {
            return true;
        }
        let result = (|| {
            sessions.authorize_projection(&view.negotiated.client, view.negotiated.view_id)?;
            let _allocation = sessions.reserve_view_memory(
                &view.negotiated.client,
                view.negotiated.view_id,
                decoded + 3 * MAX_ENVELOPE_BYTES as usize,
            )?;
            let mut snapshot = snapshot.clone();
            if view.negotiated.profiles.contains("semantics.only.v1") {
                semantic_projection(&mut snapshot);
            } else if source.is_none() {
                snapshot.required_features = vec![
                    "mirror2d.core.v1".into(),
                    "semantics.core.v1".into(),
                    "content.no-raster.v1".into(),
                ];
            }
            let closure = retain_closure(&snapshot, source, sessions, &view.negotiated)?;
            let bytes = pb::Envelope {
                protocol_version: 1,
                session_id: view.negotiated.session_id.to_vec(),
                epoch: view.negotiated.epoch.to_vec(),
                view_id: view.negotiated.view_id.to_vec(),
                kind: pb::MessageKind::Snapshot as i32,
                payload: snapshot.encode_to_vec(),
                required_features: snapshot.required_features.clone(),
            }
            .encode_to_vec();
            if bytes.len() > view.negotiated.limits.envelope_bytes
                || snapshot.records.len() > view.negotiated.limits.records
            {
                return Err(GtkHostError::Unsupported);
            }
            let publication = sessions.snapshot_publication_payload(
                &view.negotiated.client,
                view.negotiated.view_id,
                bytes,
                Some(closure),
            )?;
            // A slow view owns one installed publication and one replacement.
            // Snapshot reset is delayed until the previous real readiness ACK;
            // old exact ACKs remain valid while source/controller work proceeds.
            view.replacement = Some(PendingSnapshot {
                publication,
                revision: snapshot.revision,
                decoded,
            });
            if view.ready {
                install_replacement(sessions, view)?;
            }
            Ok::<(), GtkHostError>(())
        })();
        if result.is_err() {
            // A client whose negotiated pool cannot hold this source projection
            // closes independently. Existing controllers/observers still publish.
            // The broker clamps this value to its current trusted monotonic clock.
            let _ = sessions.invalidate_authorization(&view.negotiated.client, 0);
            return false;
        }
        true
    });
    Ok(())
}

fn retain_closure(
    snapshot: &pb::Snapshot,
    source: Option<&ViewReceiver>,
    sessions: &SessionBroker,
    negotiated: &NegotiatedSession,
) -> Result<rui_protocol_core::session::PublicationAttachment, GtkHostError> {
    let mut charge = 8192usize;
    for digest in &snapshot.required_resources {
        let digest: ResourceDigest = digest
            .as_slice()
            .try_into()
            .map_err(|_| GtkHostError::Unsupported)?;
        let source = source.ok_or(GtkHostError::Unsupported)?;
        charge = charge
            .checked_add(
                source
                    .read_resource(digest)
                    .map_err(CaptureError::from)?
                    .len()
                    + 1024,
            )
            .ok_or(GtkHostError::Unsupported)?;
    }
    let memory =
        Arc::new(sessions.reserve_view_memory(&negotiated.client, negotiated.view_id, charge)?);
    let attachment = sessions.reserve_view_memory(&negotiated.client, negotiated.view_id, 4096)?;
    let mut resources = BTreeMap::new();
    for digest in &snapshot.required_resources {
        let digest: ResourceDigest = digest
            .as_slice()
            .try_into()
            .map_err(|_| GtkHostError::Unsupported)?;
        let source = source.ok_or(GtkHostError::Unsupported)?;
        resources.insert(
            digest,
            PublishedResource {
                offer: source
                    .resource_offer(digest)
                    .map_err(CaptureError::from)?
                    .clone(),
                bytes: Arc::from(source.read_resource(digest).map_err(CaptureError::from)?),
                _memory: memory.clone(),
            },
        );
    }
    Ok(attachment.attach(PublishedClosure {
        revision: snapshot.revision,
        resources,
        _memory: memory,
    }))
}

fn install_replacement(sessions: &mut SessionBroker, view: &mut View) -> Result<(), GtkHostError> {
    let Some(pending) = view.replacement.take() else {
        return Ok(());
    };
    // Full-view verification does not create a renderer acknowledgement. The
    // native authority remains the private, unredacted source graph.
    let staged = (|| {
        verify_publication(&mut view.consumer, &view.negotiated, &pending.publication)?;
        sessions.install_snapshot(
            &view.negotiated.client,
            view.negotiated.view_id,
            pending.revision,
            pending.decoded,
        )?;
        Ok::<(), GtkHostError>(())
    })();
    if let Err(error) = staged {
        // Consumer verification can commit before the broker's decoded-state
        // reservation fails. Close this view atomically with that failure;
        // neither old readiness nor a mismatched committed graph survives.
        let _ = sessions.invalidate_authorization(&view.negotiated.client, 0);
        view.consumer.revoke();
        view.installed = None;
        view.publication = None;
        view.ready = false;
        return Err(error);
    }
    view.installed = Some(pending.publication.clone());
    view.publication = Some(pending.publication);
    view.published_revision = pending.revision;
    view.ready = false;
    Ok(())
}

fn verify_publication(
    consumer: &mut ViewReceiver,
    negotiated: &NegotiatedSession,
    publication: &SharedPayload,
) -> Result<(), GtkHostError> {
    let closure = publication
        .attachment()
        .and_then(|a| a.get::<PublishedClosure>())
        .ok_or(GtkHostError::Unsupported)?;
    for digest in closure.resources.keys() {
        consumer
            .authorize_resource(*digest)
            .map_err(CaptureError::from)?;
    }
    let missing = if consumer.state().map_err(CaptureError::from)?.revision == closure.revision {
        // Cache-loss recovery leaves the verified live graph installed. A
        // duplicate publication intentionally cannot acknowledge unavailable
        // bytes; refill that exact live closure before retrying readiness.
        Some(
            consumer
                .missing_live_resources()
                .map_err(CaptureError::from)?,
        )
    } else {
        match consumer
            .receive_envelope((closure.revision as u128).to_be_bytes(), publication)
            .map_err(CaptureError::from)?
        {
            ReceiveOutcome::NeedResources { missing, .. } => Some(missing),
            _ => None,
        }
    };
    if let Some(missing) = missing {
        let context = ResourceContext {
            scope: ResourceScope {
                session_id: negotiated.session_id,
                epoch: negotiated.epoch,
                principal_id: negotiated.client.client_instance_id,
            },
            view_id: negotiated.view_id,
        };
        for digest in &*missing {
            let resource = closure
                .resources
                .get(digest)
                .ok_or(GtkHostError::Unsupported)?;
            if consumer
                .receive_resource_offer(context, resource.offer.clone())
                .map_err(CaptureError::from)?
                == OfferOutcome::AlreadyAvailable
            {
                continue;
            }
            for (index, chunk) in resource
                .bytes
                .chunks(negotiated.limits.chunk_bytes)
                .enumerate()
            {
                consumer
                    .resource_chunk(context, resource.offer.digest, index as u32, chunk)
                    .map_err(CaptureError::from)?;
            }
            consumer
                .complete_resource(context, resource.offer.digest)
                .map_err(CaptureError::from)?;
        }
    }
    if consumer.state().map_err(CaptureError::from)?.revision != closure.revision
        || !consumer.resources_ready().map_err(CaptureError::from)?
    {
        return Err(GtkHostError::Unsupported);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn visual_host() -> GtkCommandHost {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/gtk/whole-native-golden/whole-native");
        let budget = MemoryBudget::new(256 << 20);
        let source_memory = budget.reserve(SOURCE_MEMORY).unwrap();
        let ledger_memory = budget.reserve(4 << 20).unwrap();
        let identity_memory = budget.reserve(4 << 20).unwrap();
        let capture = NativeCapture {
            _memory: budget.reserve(ARTIFACT_MEMORY).unwrap(),
            artifact: CaptureArtifact::read(root.join("snapshot-01-000001.json")).unwrap(),
        };
        let mut source = capture.artifact.receiver().unwrap();
        capture.artifact.apply_trusted_fixture(&mut source).unwrap();
        let context = capture.artifact.context();
        let directory =
            std::env::temp_dir().join(format!("rui-visual-owner-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let pipe = NativePipe {
            fence: NativeFence::new(&budget, 1).unwrap(),
            child: ProcessCommand::new("true").spawn().unwrap(),
            socket,
            directory,
            _working: budget.reserve(1 << 20).unwrap(),
            budget: budget.clone(),
            next_request: 1,
            source_generation: 1,
            failed: false,
            diagnostics: NativeDiagnostics::default(),
        };
        let config = SessionConfig {
            max_clients: MAX_CLIENTS,
            receiver_limits: ReceiverLimits {
                view_memory_bytes: CLIENT_MEMORY,
                ..Default::default()
            },
            supported_profiles: [
                "mirror2d.core.v1",
                "semantics.core.v1",
                "content.no-raster.v1",
                "glyph.outline-single.v1",
                "semantics.only.v1",
                "text.clock.gtk-editable4224.v1",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            ..Default::default()
        };
        let composition = composition::Composition::new(&budget).unwrap();
        let mut host = GtkCommandHost {
            pipe,
            capture,
            source,
            sessions: SessionBroker::with_memory_budget(
                context.scope.session_id,
                context.scope.epoch,
                config,
                budget,
            )
            .unwrap(),
            ledger: CommandLedger::new(context.scope.epoch, LedgerLimits::default()).unwrap(),
            composition,
            views: BTreeMap::new(),
            native_lease: None,
            broker_epoch: context.scope.epoch,
            source_incarnations: [context.scope.epoch].into_iter().collect(),
            public_revision: 0,
            source_offset: 0,
            source_alive: true,
            identities: SourceIdentities::new(),
            _identity_memory: identity_memory,
            last_canvas: None,
            last_source_signal: None,
            _source_memory: source_memory,
            _ledger_memory: ledger_memory,
        };
        host.publish_all().unwrap();
        host
    }

    fn visual_client(host: &mut GtkCommandHost, id: u8) -> AuthenticatedClient {
        let profiles: BTreeSet<_> = [
            "mirror2d.core.v1",
            "semantics.core.v1",
            "content.no-raster.v1",
            "glyph.outline-single.v1",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        host.negotiate(
            VerifiedAuthorization {
                principal: format!("visual-{id}"),
                policy_version: 1,
                permissions: rui_protocol_core::session::Permissions {
                    can_view: true,
                    can_control: true,
                    actions: ["Focus".into()].into_iter().collect(),
                },
            },
            ClientHello {
                client_instance_id: [id; 16],
                supported_versions: [1].into_iter().collect(),
                supported_profiles: profiles.clone(),
                required_profiles: profiles,
                receiver_limits: ReceiverLimits::default(),
                resume: None,
            },
            0,
        )
        .unwrap()
        .client
    }

    fn advance_visual_source(host: &mut GtkCommandHost, revision: u64) {
        let next = NativeCapture {
            _memory: host.pipe.budget.reserve(ARTIFACT_MEMORY).unwrap(),
            artifact: CaptureArtifact::read(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../fixtures/gtk/whole-native-golden/whole-native/snapshot-01-{revision:06}.json"))).unwrap(),
        };
        next.artifact
            .apply_trusted_fixture(&mut host.source)
            .unwrap();
        host.capture = next;
        host.publish_all().unwrap();
    }

    #[test]
    fn stalled_visual_observer_retains_old_ack_and_closure_while_controller_advances() {
        let mut host = visual_host();
        let budget = host.memory_budget();
        let controller = visual_client(&mut host, 2);
        let observer = visual_client(&mut host, 3);
        let first = host.views[&observer.client_instance_id]
            .installed
            .clone()
            .unwrap();
        let controller_first = host.views[&controller.client_instance_id]
            .installed
            .clone()
            .unwrap();
        let old_closure = first
            .attachment()
            .unwrap()
            .get::<PublishedClosure>()
            .unwrap();
        assert!(!old_closure.resources.is_empty());
        assert!(host
            .sessions
            .acquire_control(
                &controller,
                host.views[&controller.client_instance_id]
                    .negotiated
                    .view_id,
                [8; 16],
                0
            )
            .is_err());
        host.acknowledge_applied(&controller, 1, &controller_first)
            .unwrap();
        assert!(host.presented_revision(&controller).unwrap().is_none());
        host.acknowledge_presented(&controller, 1).unwrap();
        advance_visual_source(&mut host, 2);
        advance_visual_source(&mut host, 3);
        assert_eq!(
            host.views[&controller.client_instance_id].published_revision,
            2
        );
        assert_eq!(
            host.views[&observer.client_instance_id].published_revision,
            1
        );
        assert_eq!(
            host.views[&observer.client_instance_id]
                .replacement
                .as_ref()
                .unwrap()
                .revision,
            3
        );
        assert!(host.acknowledge_applied(&observer, 3, &first).is_err());
        assert!(host
            .acknowledge_applied(&observer, 1, &controller_first)
            .is_err());
        // The exact old witness still works after two source captures. It does
        // not acquire presentation on its own or advance another view.
        host.acknowledge_applied(&observer, 1, &first).unwrap();
        assert!(host.presented_revision(&observer).unwrap().is_none());
        host.acknowledge_presented(&observer, 1).unwrap();
        assert_eq!(
            host.views[&observer.client_instance_id].published_revision,
            3
        );
        assert!(host.presented_revision(&observer).unwrap().is_none());
        assert_eq!(host.state(&observer).unwrap().revision, 3);
        assert_eq!(
            host.views[&controller.client_instance_id].published_revision,
            2
        );
        let resource = old_closure.resources.values().next().unwrap().clone();
        let retained_charge = 8192
            + old_closure
                .resources
                .values()
                .map(|resource| resource.bytes.len() + 1024)
                .sum::<usize>();
        host.source.close();
        assert_eq!(
            old_closure
                .resources
                .values()
                .next()
                .unwrap()
                .bytes
                .as_ref(),
            resource.bytes.as_ref()
        );
        host.disconnect(&controller, 0).unwrap();
        host.disconnect(&observer, 0).unwrap();
        drop(controller_first);
        drop(first);
        drop(host);
        assert_eq!(budget.used(), retained_charge);
        drop(resource);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn visual_resource_cache_loss_recovers_exact_old_closure_without_source_cache() {
        let mut host = visual_host();
        let client = visual_client(&mut host, 4);
        let other = visual_client(&mut host, 5);
        let publication = host.views[&client.client_instance_id]
            .installed
            .clone()
            .unwrap();
        let digest = *publication
            .attachment()
            .unwrap()
            .get::<PublishedClosure>()
            .unwrap()
            .resources
            .keys()
            .next()
            .unwrap();
        assert!(host
            .publication_resource(&other, &publication, digest)
            .is_err());
        assert!(host
            .publication_resource(&client, &publication, [255; 32])
            .is_err());
        host.acknowledge_applied(&client, 1, &publication).unwrap();
        host.acknowledge_presented(&client, 1).unwrap();
        let view = host.views.get_mut(&client.client_instance_id).unwrap();
        view.consumer.resource_cache_lost().unwrap();
        assert!(!view.consumer.resources_ready().unwrap());
        host.sessions
            .mark_resources_missing(&client, view.negotiated.view_id)
            .unwrap();
        view.ready = false;
        assert!(host.acknowledge_presented(&client, 1).is_err());
        advance_visual_source(&mut host, 2);
        host.source.close();
        // Resource recovery uses the retained publication capability, never
        // the now-closed source cache or an arbitrary globally known digest.
        let resource = host
            .publication_resource(&client, &publication, digest)
            .unwrap();
        assert_eq!(resource.offer.digest, digest);
        assert!(!resource.bytes.is_empty());
        let view = host.views.get_mut(&client.client_instance_id).unwrap();
        verify_publication(&mut view.consumer, &view.negotiated, &publication).unwrap();
        host.acknowledge_applied(&client, 1, &publication).unwrap();
        host.acknowledge_presented(&client, 1).unwrap();
        assert_eq!(host.state(&client).unwrap().revision, 2);
        assert!(host.presented_revision(&client).unwrap().is_none());
        host.revoke_authorization(&client, 0).unwrap();
        assert!(host
            .publication_resource(&client, &publication, digest)
            .is_err());
    }

    #[test]
    fn partial_visual_snapshot_cannot_create_presented_or_control_authority() {
        let mut host = visual_host();
        let client = visual_client(&mut host, 6);
        let first = host.views[&client.client_instance_id]
            .installed
            .clone()
            .unwrap();
        host.acknowledge_applied(&client, 1, &first).unwrap();
        host.acknowledge_presented(&client, 1).unwrap();
        let envelope = pb::Envelope::decode(
            std::fs::read(host.capture.artifact.path.with_extension("bin"))
                .unwrap()
                .as_slice(),
        )
        .unwrap();
        let mut snapshot = pb::Snapshot::decode(envelope.payload.as_slice()).unwrap();
        host.identities.project(&mut snapshot, 0).unwrap();
        snapshot.revision = 2;
        for record in &mut snapshot.records {
            match record.record.as_mut() {
                Some(pb::record::Record::Surface(surface)) => {
                    surface.version = 2;
                    surface.visual_coverage = 2;
                    surface
                        .unsupported_features
                        .push("native-unsupported-effect".into());
                }
                Some(pb::record::Record::Session(session)) => session.version = 2,
                _ => {}
            }
        }
        publish_snapshot(
            snapshot,
            host.source.state().unwrap().estimated_bytes(),
            Some(&host.source),
            &mut host.sessions,
            &mut host.views,
        )
        .unwrap();
        let second = host.views[&client.client_instance_id]
            .installed
            .clone()
            .unwrap();
        host.acknowledge_applied(&client, 2, &second).unwrap();
        assert!(host.acknowledge_presented(&client, 2).is_err());
        assert!(host.presented_revision(&client).unwrap().is_none());
        assert!(host
            .sessions
            .acquire_control(
                &client,
                host.views[&client.client_instance_id].negotiated.view_id,
                [9; 16],
                0
            )
            .is_err());
    }

    #[test]
    fn replacement_install_failure_revokes_only_its_view_after_verified_commit() {
        let mut host = visual_host();
        let client = visual_client(&mut host, 7);
        let other = visual_client(&mut host, 8);
        let old = host.views[&client.client_instance_id]
            .installed
            .clone()
            .unwrap();
        let other_old = host.views[&other.client_instance_id]
            .installed
            .clone()
            .unwrap();
        advance_visual_source(&mut host, 2);
        host.views
            .get_mut(&client.client_instance_id)
            .unwrap()
            .replacement
            .as_mut()
            .unwrap()
            .decoded = CLIENT_MEMORY + 1;
        host.acknowledge_applied(&client, 1, &old).unwrap();
        assert!(host.acknowledge_presented(&client, 1).is_err());
        assert!(host.state(&client).is_err());
        assert_eq!(
            host.views[&client.client_instance_id]
                .consumer
                .memory_used(),
            0
        );
        assert!(host.views[&client.client_instance_id].installed.is_none());
        host.acknowledge_applied(&other, 1, &other_old).unwrap();
        host.acknowledge_presented(&other, 1).unwrap();
        assert_eq!(host.state(&other).unwrap().revision, 2);
    }

    #[test]
    fn private_reply_rejects_delayed_source_generation_before_accepting_diagnostics() {
        for (generation, accepted) in [(1, false), (2, true)] {
            let budget = MemoryBudget::new((1 << 20) + 8192);
            let directory = std::env::temp_dir().join(format!(
                "rui-private-reply-{}-{generation}",
                std::process::id()
            ));
            std::fs::create_dir(&directory).unwrap();
            let (socket, mut peer) = UnixStream::pair().unwrap();
            let child = ProcessCommand::new("true").spawn().unwrap();
            let mut pipe = NativePipe {
                fence: NativeFence::new(&budget, 2).unwrap(),
                child,
                socket,
                directory: directory.clone(),
                budget: budget.clone(),
                _working: budget.reserve(1 << 20).unwrap(),
                next_request: 7,
                source_generation: 2,
                failed: false,
                diagnostics: NativeDiagnostics::default(),
            };
            let body = serde_json::json!({"request_id":7,"source_generation":generation,"status":"control",
                "artifact":null,"reason":null,"native_dispatches":9,"native_callbacks":10}).to_string();
            let writer = std::thread::spawn(move || {
                peer.write_all(&(body.len() as u32).to_be_bytes()).unwrap();
                peer.write_all(body.as_bytes()).unwrap();
            });
            assert_eq!(
                pipe.read_reply(7, Instant::now() + TIMEOUT).is_ok(),
                accepted
            );
            assert_eq!(
                pipe.diagnostics.native_dispatches,
                if accepted { 9 } else { 0 }
            );
            writer.join().unwrap();
            drop(pipe);
            assert_eq!(budget.used(), 0);
            std::fs::remove_dir(directory).unwrap();
        }
    }

    #[test]
    fn capture_token_must_match_actual_installed_wire_source_evidence() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/gtk/native-golden/semantic/snapshot-000001.json");
        let artifact = CaptureArtifact::read(path).unwrap();
        let mut source = artifact.receiver().unwrap();
        artifact.apply_trusted_fixture(&mut source).unwrap();
        let mut capture = NativeCapture {
            artifact,
            _memory: MemoryBudget::new(ARTIFACT_MEMORY)
                .reserve(ARTIFACT_MEMORY)
                .unwrap(),
        };
        verify_capture(&capture, &source).unwrap();
        capture.artifact.manifest["capture_token"] =
            serde_json::Value::String("unrelated-capture-token".into());
        assert!(verify_capture(&capture, &source).is_err());
    }

    #[test]
    fn failed_consumer_construction_invalidates_registered_session() {
        use rui_protocol_core::session::Permissions;
        let mut config = SessionConfig::default();
        config.supported_profiles.insert("semantics.only.v1".into());
        let profiles = config.supported_profiles.clone();
        let mut sessions = SessionBroker::new([1; 16], [2; 16], config).unwrap();
        let negotiated = sessions
            .negotiate(
                Some(VerifiedAuthorization {
                    principal: "controlled-fixture".into(),
                    policy_version: 1,
                    permissions: Permissions {
                        can_view: true,
                        can_control: false,
                        actions: BTreeSet::new(),
                    },
                }),
                ClientHello {
                    client_instance_id: [3; 16],
                    supported_versions: [1].into_iter().collect(),
                    supported_profiles: profiles,
                    required_profiles: ["semantics.only.v1".into()].into_iter().collect(),
                    receiver_limits: ReceiverLimits {
                        view_memory_bytes: 256,
                        ..ReceiverLimits::default()
                    },
                    resume: None,
                },
                0,
            )
            .unwrap();
        assert!(sessions
            .authorize_projection(&negotiated.client, negotiated.view_id)
            .is_ok());
        assert!(consumer_for_view(&mut sessions, &negotiated, 0).is_err());
        assert!(sessions
            .authorize_projection(&negotiated.client, negotiated.view_id)
            .is_err());
        assert!(sessions
            .acquire_control(&negotiated.client, negotiated.view_id, [4; 16], 0)
            .is_err());
    }
}
