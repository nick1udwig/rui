//! Optional GTK mirror shell. All document state remains in NetworkClient's
//! receiver; local viewport changes never generate application commands.
use crate::{
    composition::{ClockPolicy, CompositionQueue, NativeStage, GTK_EDITABLE_CLOCK_PROFILE},
    interaction::{self, CommandBinding, EditingContext},
    physical_dispatch::{self, CapturedPhysicalInput, PhysicalControl},
    physical_input::PhysicalKeyOwner,
    source_controls::{self, DeviceScope, PointerStroke, ResizeTarget},
    *,
};
use prost::Message;
use rui_protocol_core::{
    commands::{Arguments, CommandFamily},
    model::*,
    resources::{MemoryBudget, MemoryReservation},
    session::{ClientHello, ReceiverLimits, SessionConfig},
    wire::proto as core,
};
use rui_transport::{
    pairing::Credential, proto as session, ClientEvent, NetworkClient, OpenedView, TransportError,
};
use sha2::{Digest, Sha256};
use std::{
    net::SocketAddr,
    sync::mpsc::{SyncSender, TrySendError},
    time::Duration,
};
use tokio::sync::{mpsc, watch};

pub mod composition_cleanup;
pub mod gtk;
pub mod native;
mod physical_callbacks;
mod physical_continuity;
pub mod physical_io;
pub mod physical_native;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewSelection {
    #[default]
    Fixture,
    WholeSession,
}
impl ViewSelection {
    async fn open(self, client: &mut NetworkClient) -> Result<(), TransportError> {
        match self {
            Self::Fixture => client.open_view().await,
            Self::WholeSession => client.open_view_with("session", "host-canvas").await,
        }
    }
}

pub struct Options {
    pub address: SocketAddr,
    pub server_name: String,
    pub certificate: Vec<u8>,
    pub credential: Credential,
    pub client_id: [u8; 16],
    pub view_selection: ViewSelection,
    /// Local display scale only. It never resizes the authoritative source.
    pub initial_scale: f64,
    pub auto_control: bool,
    pub smoke_frames: Option<u64>,
    pub title: &'static str,
    pub observations: Option<watch::Sender<DesktopObservation>>,
    pub shutdown_signal: Option<watch::Receiver<bool>>,
}
/// Optional bounded latest diagnostics for local integration tests. It contains
/// no application text, credentials or authority tokens and cannot acknowledge.
#[derive(Clone, Copy, Debug, Default)]
pub struct DesktopObservation {
    pub binding: Option<ObservationBinding>,
    pub displayed: Option<DisplayedObservation>,
    pub has_control: bool,
    pub input_ready: bool,
    pub physical_ready: bool,
    pub physical_notice: Option<&'static str>,
    pub render_notice: Option<&'static str>,
    pub control_revoked: bool,
    pub editing_target: Option<u64>,
    pub composition: CompositionObservation,
    pub wire_composition: bool,
    /// Actual selected optional profile, recorded without handshake credentials.
    pub gtk_composition_clock: bool,
    pub preedit_visible: bool,
    pub typing_pending: usize,
    pub typing_notice: TypingNotice,
    pub notice_visible: bool,
    pub closed: bool,
    pub local_controls: Option<LocalControls>,
    /// Bounded, opt-in count of actual GTK Request control button callbacks.
    pub control_requests: u64,
    pub last_stroke: Option<StrokeObservation>,
    pub stroke_notice: Option<&'static str>,
    pub stroke_notice_visible: bool,
    pub last_action: Option<ActionObservation>,
    pub failure_class: Option<&'static str>,
    pub render_timing: RenderTimingObservation,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct CompositionObservation {
    pub begins: u64,
    pub updates: u64,
    pub commits: u64,
    pub cancels: u64,
}
/// Optional bounded scheduling counters. A tick is a render opportunity, while
/// GTK after-paint remains the independent actual presentation callback.
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderTimingObservation {
    pub accepted_ticks: u64,
    pub late_ticks: u64,
    pub elapsed_missed_periods: u64,
    pub rendered_frames: u64,
    pub work_over_budget: u64,
    pub max_work_us: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct ActionObservation {
    pub started_us: i64,
    pub observed_revision: u64,
    pub kind: ActionKind,
}
#[derive(Clone, Copy, Debug)]
pub enum ActionKind {
    SemanticClick,
    SourceScroll,
    SourceResize,
    StrokeRelease,
}
#[derive(Clone, Copy, Debug)]
pub struct ObservationBinding {
    pub client: [u8; 16],
    pub epoch: [u8; 16],
    pub view: [u8; 16],
}
#[derive(Clone, Copy, Debug)]
pub struct DisplayedObservation {
    pub stamp: Stamp,
    pub canvas_origin: Point,
    pub window_scale: i32,
    pub frame_time_us: i64,
    pub refresh_interval_us: i64,
    pub predicted_presentation_us: i64,
    pub after_paint_time_us: i64,
}
#[derive(Clone, Copy, Debug)]
pub struct LocalControls {
    pub request_control: Point,
    pub release_control: Point,
    pub resize: Point,
    pub width: Point,
    pub height: Point,
    pub scroll: Point,
    pub scroll_y: Point,
    pub reconnect: Point,
    pub cache: Point,
}
#[derive(Clone, Copy, Debug)]
pub struct StrokeObservation {
    pub command_sequence: u64,
    pub event_count: usize,
    pub events_digest: [u8; 32],
}
#[derive(Clone, Copy, Debug)]
pub struct Stamp {
    pub generation: u64,
    pub serial: u64,
    pub revision: u64,
    pub viewport: Viewport,
    pub pixel_digest: [u8; 32],
    pub records_digest: Option<[u8; 32]>,
}
/// Streaming, bounded normalized record fingerprint for local diagnostic
/// comparison. It never retains or prints serialized document content.
pub fn records_digest(state: &State) -> Option<[u8; 32]> {
    struct Writer {
        hash: Sha256,
        bytes: usize,
    }
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(bytes.len())
                .filter(|n| *n <= 16 * 1024 * 1024)
                .ok_or_else(|| std::io::Error::other("diagnostic record ceiling"))?;
            self.hash.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer {
        hash: Sha256::new(),
        bytes: 0,
    };
    serde_json::to_writer(&mut writer, &state.records).ok()?;
    Some(writer.hash.finalize().into())
}
pub enum UiCommand {
    Acquire,
    Release,
    Reconnect,
    LoseResources,
    Presented(Stamp),
    Click {
        stamp: Stamp,
        point: Point,
    },
    Commit {
        stamp: Stamp,
        context: EditingContext,
        text: String,
        _memory: MemoryReservation,
    },
    Composition {
        stamp: Stamp,
        context: EditingContext,
        id: [u8; 16],
        operation: rui_protocol_core::commands::CompositionOperation,
        _memory: MemoryReservation,
    },
    SelectAll {
        stamp: Stamp,
    },
    Stroke {
        stamp: Stamp,
        stroke: PointerStroke,
    },
    Physical(CapturedPhysicalInput),
    ResizeSource {
        stamp: Stamp,
        target: ResizeTarget,
        width: f64,
        height: f64,
    },
    ScrollSource {
        stamp: Stamp,
        target: u64,
        x: f64,
        y: f64,
    },
    Quit,
}
pub enum UiEvent {
    Frame {
        stamp: Stamp,
        pixels: RgbaFrame,
        routing: Option<std::sync::Arc<physical_dispatch::PresentedRouting>>,
    },
    Status(&'static str),
    Ready(MemoryBudget),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    Live,
    Suspended,
    Closed,
    Quit,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TypingNotice {
    #[default]
    None,
    Cancelled,
    Limit,
    Rejected,
}
impl TypingNotice {
    pub fn message(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Cancelled => Some("Editing context changed; pending typing cancelled."),
            Self::Limit => {
                Some("Typing limit reached; pending input cancelled. Select the target to resume.")
            }
            Self::Rejected => {
                Some("Remote text edit rejected or unconfirmed; pending input cancelled.")
            }
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiState {
    pub generation: u64,
    pub editing_generation: u64,
    pub lifecycle: Lifecycle,
    pub editing: Option<EditingContext>,
    pub typing_notice: TypingNotice,
    pub wire_composition: bool,
    pub composition_control: Option<PhysicalControl>,
    pub raw_input: bool,
    pub physical_control: Option<PhysicalControl>,
    pub physical_keyboard: Option<PhysicalKeyOwner>,
    pub physical_ready: bool,
    pub physical_notice: Option<&'static str>,
    pub render_notice: Option<&'static str>,
    pub source_resize: Option<ResizeTarget>,
    pub source_scroll: Option<u64>,
}
impl Default for UiState {
    fn default() -> Self {
        Self {
            generation: 0,
            editing_generation: 0,
            lifecycle: Lifecycle::Live,
            editing: None,
            typing_notice: TypingNotice::None,
            wire_composition: false,
            composition_control: None,
            raw_input: false,
            physical_control: None,
            physical_keyboard: None,
            physical_ready: false,
            physical_notice: None,
            render_notice: None,
            source_resize: None,
            source_scroll: None,
        }
    }
}
impl UiState {
    pub fn clear(&mut self, lifecycle: Lifecycle) {
        if matches!(self.lifecycle, Lifecycle::Closed | Lifecycle::Quit) {
            return;
        }
        match self.generation.checked_add(1) {
            Some(generation) => {
                self.generation = generation;
                self.lifecycle = lifecycle;
            }
            None => self.lifecycle = Lifecycle::Quit,
        }
        self.editing = None;
        self.wire_composition = false;
        self.composition_control = None;
        self.raw_input = false;
        self.physical_control = None;
        self.physical_keyboard = None;
        self.physical_ready = false;
        self.source_resize = None;
        self.source_scroll = None;
        self.render_notice = None;
    }
    pub fn resume(&mut self) -> bool {
        if self.lifecycle == Lifecycle::Suspended {
            self.lifecycle = Lifecycle::Live;
        }
        self.lifecycle == Lifecycle::Live
    }
    pub fn input_control(&self) -> Option<PhysicalControl> {
        self.physical_control.or(self.composition_control)
    }
    pub fn accepts(&self, stamp: Stamp) -> bool {
        self.lifecycle == Lifecycle::Live && self.generation == stamp.generation
    }
}

fn retire_editing_owner(
    typing: &mut crate::typing::TypingQueue,
    composition: &mut CompositionQueue,
    selected: &mut Option<u64>,
    ui: &mut UiState,
) -> Result<(), ()> {
    typing.cancel();
    composition.cancel();
    *selected = None;
    ui.editing = None;
    ui.composition_control = None;
    ui.editing_generation = ui.editing_generation.checked_add(1).ok_or(())?;
    Ok(())
}
fn current_revocation(lease: Option<&Lease>, notice: &session::ControlRevoked) -> bool {
    lease.is_some_and(|lease| {
        lease.wire.lease_id == notice.lease_id && lease.wire.generation == notice.generation
    })
}
fn composition_policy(client: &NetworkClient) -> ClockPolicy {
    if client
        .profiles
        .iter()
        .any(|profile| profile == GTK_EDITABLE_CLOCK_PROFILE)
    {
        ClockPolicy::GtkEditable4224
    } else {
        ClockPolicy::Strict
    }
}
fn composition_wire(
    command: rui_protocol_core::commands::TextCompositionCommand,
) -> core::TextComposition {
    use rui_protocol_core::commands::CompositionOperation as Operation;
    let header = command.header;
    core::TextComposition {
        header: Some(core::CommandHeader {
            epoch: header.epoch.to_vec(),
            view_id: header.view_id.to_vec(),
            client_instance_id: header.client_instance_id.to_vec(),
            command_seq: header.command_seq,
            control_lease_id: header.control_lease_id.to_vec(),
            control_generation: header.control_generation,
            observed_view_revision: header.observed_view_revision,
        }),
        target_id: command.target_id,
        text_session_id: command.text_session_id.to_vec(),
        focus_generation: command.focus_generation,
        base_text_version: command.base_text_version,
        operation: Some(match command.operation {
            Operation::Begin(range) => {
                core::text_composition::Operation::Begin(core::ScalarRange {
                    start: range.start,
                    end: range.end,
                })
            }
            Operation::UpdatePreedit { text, selection } => {
                core::text_composition::Operation::UpdatePreedit(core::Preedit {
                    text,
                    selection: Some(core::ScalarRange {
                        start: selection.start,
                        end: selection.end,
                    }),
                })
            }
            Operation::CommitText { text } => {
                core::text_composition::Operation::CommitText(core::CommitText { text })
            }
            Operation::Cancel => core::text_composition::Operation::Cancel(core::EmptyArguments {}),
        }),
    }
}
struct Lease {
    wire: session::ControlGranted,
    _memory: MemoryReservation,
}
fn command_binding(
    client: &NetworkClient,
    client_id: [u8; 16],
    lease: Option<&Lease>,
    sequence: u64,
) -> Option<CommandBinding> {
    let lease = lease?;
    Some(CommandBinding {
        epoch: client.epoch,
        view: client.view_id,
        client: client_id,
        lease: lease.wire.lease_id.as_slice().try_into().ok()?,
        generation: lease.wire.generation,
        sequence,
    })
}
async fn submit_semantic(
    client: &mut NetworkClient,
    domain: rui_protocol_core::commands::SemanticCommand,
) -> Result<(), TransportError> {
    if domain.command_seq != client.commands().next_sequence() {
        return Err(TransportError::Protocol);
    }
    submit_wire(client, CommandFamily::Semantic, &wire_command(domain)).await
}
async fn submit_wire<T: Message>(
    client: &mut NetworkClient,
    family: CommandFamily,
    wire: &T,
) -> Result<(), TransportError> {
    client.submit_encoded(family, wire).await
}
fn recoverable(error: &TransportError) -> bool {
    matches!(
        error,
        TransportError::Io
            | TransportError::WebSocket
            | TransportError::Closed
            | TransportError::Deadline
            | TransportError::ProducerBusy
    )
}
fn error_class(error: &TransportError) -> &'static str {
    match error {
        TransportError::ProducerBusy => "ProducerBusy",
        TransportError::Tls => "Tls",
        TransportError::Pin => "Pin",
        TransportError::Io => "Io",
        TransportError::WebSocket => "WebSocket",
        TransportError::Limit => "Limit",
        TransportError::Authentication => "Authentication",
        TransportError::Entropy => "Entropy",
        TransportError::Protocol => "Protocol",
        TransportError::Binding => "Binding",
        TransportError::Closed => "Closed",
        TransportError::Deadline => "Deadline",
        TransportError::Authority => "Authority",
        TransportError::Receiver => "Receiver",
    }
}
/// Reauthentication preserves the retained reducer, and never restores a lease.
/// A failed attempt waits for an explicit bounded retry. Stale UI input drops
/// while disconnected; shutdown can cancel every network wait.
#[allow(clippy::too_many_arguments)]
async fn recover_connection(
    client: NetworkClient,
    options: &Options,
    hello: &ClientHello,
    commands: &mut mpsc::Receiver<UiCommand>,
    events: &SyncSender<UiEvent>,
    shutdown: &mut watch::Receiver<bool>,
) -> Option<(NetworkClient, bool, Option<OpenedView>)> {
    let mut retained = client.into_retained();
    loop {
        let attempt = tokio::select! {
            _=shutdown.changed()=>return None,
            result=retained.reconnect(options.address,options.server_name.clone(),options.certificate.clone(),&options.credential,hello.clone())=>result,
        };
        if let Ok(mut client) = attempt {
            {
                let mut applied = false;
                let mut opened = None;
                let recovery = async {
                    options.view_selection.open(&mut client).await?;
                    // Old-lease exact retries either recover a retained result
                    // or consume a rejection. They cannot restore source input.
                    while !client.commands().is_empty() {
                        let sequence = client.commands().commands().next().unwrap().sequence();
                        client.retry_command(sequence).await?;
                        for index in 0..1024 {
                            let result = client.receive().await?;
                            match result {
                                ClientEvent::CommandResult(r) if r.command_seq == sequence => {
                                    if client
                                        .commands()
                                        .commands()
                                        .any(|c| c.sequence() == sequence)
                                    {
                                        return Err(TransportError::Authority);
                                    }
                                    break;
                                }
                                ClientEvent::ControlRevoked(_) => {}
                                ClientEvent::ViewOpened(metadata) => opened = Some(metadata),
                                ClientEvent::ResumeAccepted(_)
                                | ClientEvent::ResetRequired(_)
                                | ClientEvent::ResourceAvailable => {}
                                ClientEvent::ResourcesNeeded { .. } => applied = false,
                                ClientEvent::Applied { .. } => applied = true,
                                _ => return Err(TransportError::Protocol),
                            }
                            if index == 1023 {
                                return Err(TransportError::Limit);
                            }
                        }
                    }
                    client.flush_pending().await
                };
                let recovered = tokio::select! {
                    _=shutdown.changed()=>return None,
                    result=tokio::time::timeout(Duration::from_secs(15),recovery)=>result.unwrap_or(Err(TransportError::Deadline)),
                };
                if recovered.is_ok() {
                    return Some((client, applied, opened));
                }
                status(
                    events,
                    "Reconnect interrupted; pending outcomes remain unconfirmed.",
                );
            }
            retained = client.into_retained();
        } else {
            status(
                events,
                "Reconnect failed; check approval and identity, then retry.",
            );
        }
        loop {
            tokio::select! {
                _=shutdown.changed()=>return None,
                command=commands.recv()=>match command {
                    Some(UiCommand::Reconnect)=>break,
                    Some(UiCommand::Quit)|None=>return None,
                    Some(_)=>status(events,"Disconnected; reconnect before sending input."),
                }
            }
        }
    }
}
use rui_client_core::wire_command::wire_command;

fn deliver(tx: &SyncSender<UiEvent>, pending: &mut Option<UiEvent>) -> bool {
    if let Some(event) = pending.take() {
        match tx.try_send(event) {
            Ok(()) => {}
            Err(TrySendError::Full(event)) => *pending = Some(event),
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
    true
}
fn status(tx: &SyncSender<UiEvent>, text: &'static str) {
    let _ = tx.try_send(UiEvent::Status(text));
}
fn request_id(counter: &mut u64) -> Option<[u8; 16]> {
    let value = *counter;
    *counter = value.checked_add(1)?;
    let mut id = [0; 16];
    id[..8].copy_from_slice(b"RUIreq01");
    id[8..].copy_from_slice(&value.to_be_bytes());
    Some(id)
}

/// Runs on a Tokio worker, never GTK's main thread. select! requires the
/// transport's cancellation-safe receive contract, including retained acks.
pub async fn worker(
    options: Options,
    commands: mpsc::Receiver<UiCommand>,
    viewport: watch::Receiver<Viewport>,
    events: SyncSender<UiEvent>,
    ui: watch::Sender<UiState>,
    shutdown: watch::Receiver<bool>,
) {
    let (_priority, receiver) = watch::channel(None);
    worker_with_priority(options, commands, viewport, events, ui, shutdown, receiver).await;
}

pub async fn worker_with_priority(
    options: Options,
    mut commands: mpsc::Receiver<UiCommand>,
    mut viewport: watch::Receiver<Viewport>,
    events: SyncSender<UiEvent>,
    ui: watch::Sender<UiState>,
    mut shutdown: watch::Receiver<bool>,
    mut priority: watch::Receiver<Option<PhysicalControl>>,
) {
    let mut ui_state = UiState::default();
    let mut grant_serial = 0u64;
    let mut automatic_control = options.auto_control;
    let mut safety_stopped = false;
    let mut profiles = SessionConfig::default().supported_profiles;
    profiles.insert("glyph.outline-single.v1".into());
    profiles.insert(CURSOR_PROFILE.into());
    profiles.insert(GTK_EDITABLE_CLOCK_PROFILE.into());
    let hello = ClientHello {
        client_instance_id: options.client_id,
        supported_versions: [1].into_iter().collect(),
        supported_profiles: profiles,
        required_profiles: SessionConfig::default().required_profiles,
        receiver_limits: ReceiverLimits::default(),
        resume: None,
    };
    let mut client = match NetworkClient::connect(
        options.address,
        options.server_name.clone(),
        options.certificate.clone(),
        &options.credential,
        hello.clone(),
    )
    .await
    {
        Ok(c) => c,
        Err(error) => {
            eprintln!("RUI connection failed: {error}");
            if let Some(observations) = &options.observations {
                observations.send_modify(|o| o.failure_class = Some(error_class(&error)));
            }
            status(
                &events,
                "Connection failed; check approval and pinned identity.",
            );
            ui_state.clear(Lifecycle::Closed);
            ui.send_replace(ui_state);
            return;
        }
    };
    let mut budget = client.receiver().memory_budget();
    if let Some(observations) = &options.observations {
        observations.send_modify(|o| {
            o.binding = Some(ObservationBinding {
                client: options.client_id,
                epoch: client.epoch,
                view: client.view_id,
            })
        });
    }
    let mut _queue_memory = match budget.reserve(32 * std::mem::size_of::<UiCommand>() + 8192) {
        Ok(m) => m,
        Err(_) => {
            status(&events, "Memory limit reached.");
            ui_state.clear(Lifecycle::Closed);
            ui.send_replace(ui_state);
            return;
        }
    };
    let mut renderer = match Renderer::new_with_profile(
        RenderLimits::default(),
        budget.clone(),
        RenderProfile {
            cursor_vectors: client
                .profiles
                .iter()
                .any(|profile| profile == CURSOR_PROFILE),
        },
    ) {
        Ok(r) => r,
        Err(_) => {
            status(&events, "Pinned renderer is unavailable.");
            ui_state.clear(Lifecycle::Closed);
            ui.send_replace(ui_state);
            return;
        }
    };
    let mut pending = Some(UiEvent::Ready(budget.clone()));
    let mut initial_reconnect = match options.view_selection.open(&mut client).await {
        Ok(()) => false,
        Err(error) if recoverable(&error) => true,
        Err(_) => {
            status(&events, "View could not be opened.");
            ui_state.clear(Lifecycle::Closed);
            ui.send_replace(ui_state);
            return;
        }
    };
    // Sixty render opportunities per second. Actual acknowledgement remains
    // tied to GTK's after-paint callback, not this scheduling timer.
    let mut tick = tokio::time::interval(Duration::from_nanos(16_666_667));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut ping = tokio::time::interval(Duration::from_secs(2));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut dirty = false;
    let mut serial = 0u64;
    let mut issued = None;
    let mut issued_routing = None;
    let mut presented: Option<Stamp> = None;
    let mut presented_routing = None;
    let mut lease: Option<Lease> = None;
    let mut sequence = 1u64;
    let mut selected = None;
    let mut auto_acquired = false;
    let mut presented_frames = 0u64;
    let mut requests = 1u64;
    let mut awaiting_resources = false;
    let mut render_timing = RenderTimingObservation::default();
    let mut opened: Option<OpenedView> = None;
    let mut typing = match crate::typing::TypingQueue::new_with_policy(
        budget.clone(),
        composition_policy(&client),
    ) {
        Ok(queue) => queue,
        Err(_) => {
            ui_state.clear(Lifecycle::Closed);
            ui.send_replace(ui_state);
            return;
        }
    };
    let mut composition = match CompositionQueue::new(budget.clone(), composition_policy(&client)) {
        Ok(queue) => queue,
        Err(_) => {
            ui_state.clear(Lifecycle::Closed);
            ui.send_replace(ui_state);
            return;
        }
    };
    let mut composition_deadline = None::<tokio::time::Instant>;
    let mut physical = match physical_continuity::Continuity::new(&budget) {
        Ok(value) => value,
        Err(()) => {
            ui_state.clear(Lifecycle::Closed);
            ui.send_replace(ui_state);
            return;
        }
    };
    'connection: loop {
        let mut reconnect = std::mem::take(&mut initial_reconnect);
        loop {
            if reconnect {
                break;
            }
            let binding = command_binding(&client, options.client_id, lease.as_ref(), sequence);
            let raw_editing = match (selected, lease.as_ref(), presented) {
                (Some(target), Some(current), Some(stamp))
                    if ui_state.accepts(stamp)
                        && current
                            .wire
                            .allowed_actions
                            .iter()
                            .any(|a| a == "ReplaceText" || a == "TextComposition") =>
                {
                    current
                        .wire
                        .lease_id
                        .as_slice()
                        .try_into()
                        .ok()
                        .and_then(|lease_id| {
                            client.receiver().state().ok().and_then(|s| {
                                interaction::editing_context(
                                    s,
                                    target,
                                    &CommandBinding {
                                        epoch: client.epoch,
                                        view: client.view_id,
                                        client: options.client_id,
                                        lease: lease_id,
                                        generation: current.wire.generation,
                                        sequence,
                                    },
                                )
                            })
                        })
                }
                _ => None,
            };
            if typing.context().is_some() {
                let valid = match (raw_editing, binding, client.receiver().state()) {
                    (Some(_), Some(binding), Ok(state)) => typing.observe(state, &binding).is_ok(),
                    _ => false,
                };
                if !valid {
                    typing.cancel();
                    composition.cancel();
                    composition_deadline = None;
                    selected = None;
                    ui_state.typing_notice = TypingNotice::Cancelled;
                    ui_state.editing_generation = match ui_state.editing_generation.checked_add(1) {
                        Some(v) => v,
                        None => break,
                    };
                    status(
                        &events,
                        "Editing context changed; pending typing cancelled.",
                    );
                }
            }
            if composition.context().is_some() {
                let valid = match (raw_editing, binding, client.receiver().state()) {
                    (Some(_), Some(binding), Ok(state)) => {
                        composition.observe(state, &binding).is_ok()
                    }
                    _ => false,
                };
                if !valid {
                    composition.cancel();
                    ui_state.typing_notice = TypingNotice::Cancelled;
                    safety_stopped = true;
                    reconnect = true;
                    status(&events, "Composition context changed; control stopped. Request fresh control after recovery.");
                    break;
                }
            }
            if composition.pending_count() == 0 {
                composition_deadline = None;
            }
            if composition_deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
            {
                composition.cancel();
                ui_state.typing_notice = TypingNotice::Rejected;
                safety_stopped = true;
                reconnect = true;
                status(&events, "Composition was not confirmed; control stopped. Request fresh control after recovery.");
                break;
            }
            ui_state.editing = if selected.is_some() {
                composition.context().or(typing.context()).or(raw_editing)
            } else {
                None
            };
            let granted = |action: &str| {
                lease
                    .as_ref()
                    .is_some_and(|lease| lease.wire.allowed_actions.iter().any(|a| a == action))
            };
            let input_ready = !awaiting_resources
                && presented.is_some_and(|stamp| {
                    ui_state.accepts(stamp)
                        && client
                            .receiver()
                            .state()
                            .is_ok_and(|s| s.revision == stamp.revision)
                });
            let device = opened
                .as_ref()
                .filter(|v| v.input_devices.len() == 1)
                .map(|v| &v.input_devices[0]);
            let host_physical = device.is_some_and(|d| {
                d.input_mode == "raw.host.v1"
                    && d.text_owner == "HOST_PHYSICAL"
                    && d.key_namespace == "linux.evdev"
            });
            ui_state.wire_composition = !host_physical
                && granted("TextComposition")
                && opened.as_ref().is_some_and(|view| {
                    view.granted_input_modes
                        .iter()
                        .any(|mode| mode == "text.client.v1")
                });
            ui_state.composition_control = (ui_state.wire_composition && grant_serial > 0)
                .then_some(PhysicalControl {
                    connection_generation: ui_state.generation,
                    grant_serial,
                });
            ui_state.physical_control = (host_physical
                && granted("RawInputBatch")
                && grant_serial > 0)
                .then_some(PhysicalControl {
                    connection_generation: ui_state.generation,
                    grant_serial,
                });
            ui_state.physical_ready = ui_state.physical_control.is_some()
                && !awaiting_resources
                && client.receiver().resources_ready().unwrap_or(false)
                && presented.is_some_and(|stamp| {
                    ui_state.accepts(stamp)
                        && stamp.revision > 0
                        && client
                            .receiver()
                            .state()
                            .is_ok_and(|s| s.revision >= stamp.revision)
                });
            ui_state.physical_keyboard = ui_state.physical_control.and_then(|control| {
                client
                    .receiver()
                    .state()
                    .ok()
                    .and_then(|s| physical_dispatch::keyboard_owner(s, control))
            });
            if physical.held()
                && (!ui_state.physical_ready
                    || !ui_state.physical_control.is_some_and(|control| {
                        client
                            .receiver()
                            .state()
                            .is_ok_and(|s| physical.valid(s, control))
                    }))
            {
                safety_stopped = true;
                reconnect = true;
                status(&events,"Physical source context changed; control stopped. Request fresh control after recovery.");
                break;
            }
            ui_state.raw_input = granted("RawInputBatch")
                && input_ready
                && device.is_some_and(|d| {
                    d.input_mode == "raw.pointer.v1" && d.text_owner == "CLIENT_TEXT"
                });
            ui_state.source_resize = if input_ready && granted("ResizeRequest") {
                client
                    .receiver()
                    .state()
                    .ok()
                    .and_then(|s| source_controls::resize_target(s).ok())
            } else {
                None
            };
            ui_state.source_scroll = if input_ready && granted("ScrollTo") {
                selected.filter(|target|client.receiver().state().ok().and_then(|s|s.records.get(target)).is_some_and(|r|matches!(r,Record::Semantic(n) if n.actions.iter().any(|a|a.id=="ScrollTo"&&a.schema_version==1))))
            } else {
                None
            };
            if *ui.borrow() != ui_state {
                ui.send_replace(ui_state);
            }
            if let Some(observations) = &options.observations {
                observations.send_modify(|o| {
                    o.typing_pending = typing.pending_count() + composition.pending_count();
                    o.wire_composition = ui_state.wire_composition;
                    o.gtk_composition_clock =
                        matches!(composition_policy(&client), ClockPolicy::GtkEditable4224);
                    o.has_control = lease.is_some();
                    o.input_ready = input_ready;
                    o.physical_ready = ui_state.physical_ready;
                    o.physical_notice = ui_state.physical_notice;
                    o.render_notice = ui_state.render_notice;
                });
            }
            if !deliver(&events, &mut pending) {
                break;
            }
            if let (Some(binding), Ok(state)) = (binding, client.receiver().state()) {
                if !awaiting_resources
                    && presented.is_some_and(|stamp| {
                        ui_state.accepts(stamp) && stamp.revision == state.revision
                    })
                {
                    match composition.start(state, &binding, presented.map(|stamp| stamp.revision))
                    {
                        Ok(Some(prepared)) => {
                            composition_deadline =
                                Some(tokio::time::Instant::now() + Duration::from_secs(15));
                            let wire = composition_wire(prepared.command);
                            if let Err(error) = physical_io::guarded(
                                submit_wire(&mut client, CommandFamily::TextComposition, &wire),
                                ui_state.input_control(),
                                &mut priority,
                                &mut safety_stopped,
                            )
                            .await
                            {
                                reconnect = recoverable(&error);
                                break;
                            }
                            sequence = client.commands().next_sequence();
                            drop(prepared.memory);
                            continue;
                        }
                        Ok(None) => {}
                        Err(_) => {
                            composition.cancel();
                            ui_state.typing_notice = TypingNotice::Limit;
                            safety_stopped = true;
                            reconnect = true;
                            status(&events, "Composition limit reached; control stopped. Request fresh control after recovery.");
                            break;
                        }
                    }
                    match typing.start(sequence, state, &binding) {
                        Ok(Some(prepared)) => {
                            let domain = match interaction::captured_text_command(
                                state,
                                prepared.context,
                                prepared.text,
                                binding,
                            ) {
                                Ok(command) => command,
                                Err(_) => {
                                    typing.cancel();
                                    composition.cancel();
                                    composition_deadline = None;
                                    selected = None;
                                    ui_state.typing_notice = TypingNotice::Cancelled;
                                    status(&events, "Pending typing cancelled before dispatch.");
                                    continue;
                                }
                            };
                            if let Err(error) = physical_io::guarded(
                                submit_semantic(&mut client, domain),
                                ui_state.input_control(),
                                &mut priority,
                                &mut safety_stopped,
                            )
                            .await
                            {
                                reconnect = recoverable(&error);
                                break;
                            }
                            sequence = client.commands().next_sequence();
                            drop(prepared.memory);
                        }
                        Ok(None) => {}
                        Err(_) => {
                            typing.cancel();
                            composition.cancel();
                            composition_deadline = None;
                            selected = None;
                            ui_state.typing_notice = TypingNotice::Limit;
                            status(
                                &events,
                                "Typing limit or context changed; pending input cancelled.",
                            );
                            continue;
                        }
                    }
                }
            }
            tokio::select! {
                biased;
                changed=priority.changed(), if ui_state.input_control().is_some() || priority.has_changed().is_ok()=>{
                    if ui_state.input_control().is_some() && (changed.is_err() || *priority.borrow_and_update()==ui_state.input_control()) {
                        safety_stopped=true;reconnect=true;
                        status(&events,"Input stopped; request fresh control after recovery.");break;
                    }
                },
                _=shutdown.changed()=>{ui_state.clear(Lifecycle::Quit);break;},
                scheduled=tick.tick()=>{
                    render_timing.accepted_ticks=render_timing.accepted_ticks.saturating_add(1);
                    let lateness=tokio::time::Instant::now().saturating_duration_since(scheduled).as_nanos();
                    if lateness>16_666_667 {render_timing.late_ticks=render_timing.late_ticks.saturating_add(1);render_timing.elapsed_missed_periods=render_timing.elapsed_missed_periods.saturating_add((lateness/16_666_667).min(u128::from(u64::MAX)) as u64);}
                    if dirty && !awaiting_resources && pending.is_none() && client.receiver().resources_ready().unwrap_or(false) {
                        let work_start=std::time::Instant::now();
                        let v=*viewport.borrow();
                        match client.receiver().state().map_err(|_|crate::RenderError::Invalid).and_then(|state|renderer.render_view(state,v,client.receiver())) {
                            Ok(frame)=>match frame.into_rgba8_over(Color{red:1.0,green:1.0,blue:1.0,alpha:1.0}) {
                                Ok(pixels)=>{ui_state.render_notice=None;if !ui_state.resume() {break;}ui.send_replace(ui_state);serial=match serial.checked_add(1) {Some(v)=>v,None=>break};let pixel_digest=if options.observations.is_some(){Sha256::digest(pixels.as_ref()).into()}else{[0;32]};let records_digest=if options.observations.is_some(){client.receiver().state().ok().and_then(records_digest)}else{None};let stamp=Stamp{generation:ui_state.generation,serial,revision:pixels.revision,viewport:pixels.viewport,pixel_digest,records_digest};issued=Some(stamp);let routing=client.receiver().state().ok().and_then(|state|physical_dispatch::PresentedRouting::capture(state,&budget).ok());issued_routing=routing.clone();pending=Some(UiEvent::Frame{stamp,pixels,routing});dirty=false;},
                                Err(_)=>{if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some("display-memory"));}pending=None;issued=None;issued_routing=None;presented=None;presented_routing=None;ui_state.clear(Lifecycle::Suspended);ui_state.render_notice=Some("Display allocation limit reached. Reduce the local viewport size.");ui.send_replace(ui_state);status(&events,"Display allocation limit reached.");dirty=false;},
                            },
                            Err(error)=>{if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(match error {crate::RenderError::Memory(_)=>"render-memory",crate::RenderError::Limit(_)=>"render-limit",crate::RenderError::IncompleteSurface(_)=>"render-coverage",crate::RenderError::UnsupportedCursor=>"render-cursor",_=>"render"}));}pending=None;issued=None;issued_routing=None;presented=None;presented_routing=None;ui_state.clear(Lifecycle::Suspended);ui_state.render_notice=Some("Scene cannot be rendered within this profile and its limits.");ui.send_replace(ui_state);status(&events,"Scene cannot be rendered within this profile and its limits.");dirty=false;},
                        }
                        let elapsed=work_start.elapsed();render_timing.rendered_frames=render_timing.rendered_frames.saturating_add(1);
                        render_timing.max_work_us=render_timing.max_work_us.max(elapsed.as_micros().min(u128::from(u64::MAX)) as u64);
                        if elapsed.as_nanos()>16_666_667 {render_timing.work_over_budget=render_timing.work_over_budget.saturating_add(1);}
                    }
                    if let Some(observations)=&options.observations {observations.send_modify(|o|o.render_timing=render_timing);}
                },
                _=ping.tick()=>{
                    if let Err(error)=physical_io::guarded(client.send(session::Kind::Ping,&session::Ping{nonce:serial}),ui_state.input_control(),&mut priority,&mut safety_stopped).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}
                    if let Some(current)=&lease {
                        let Some(request)=request_id(&mut requests) else {break;};
                        if let Err(error)=physical_io::guarded(client.send(session::Kind::RenewControl,&session::RenewControl{request_id:request.to_vec(),lease_id:current.wire.lease_id.clone(),generation:current.wire.generation}),ui_state.input_control(),&mut priority,&mut safety_stopped).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}
                    }
                },
                changed=viewport.changed()=>{if changed.is_err() {break;}dirty=true;},
                event=client.receive()=>match event {
                    Ok(ClientEvent::ViewOpened(metadata))=>{if opened.is_some() {break;}opened=Some(metadata);},
                    Ok(ClientEvent::Applied{..})=>{awaiting_resources=false;dirty=true;status(&events,"View applied; waiting for display.");},
                    Ok(ClientEvent::ResourcesNeeded{..})=>{presented=None;presented_routing=None;issued=None;issued_routing=None;awaiting_resources=true;status(&events,"Waiting for verified resources; typing remains queued.");},
                    Ok(ClientEvent::ControlGranted(granted))=>{
                        if let Some(observations)=&options.observations {observations.send_modify(|o|o.control_revoked=false);}
                        let charge=2048+granted.allowed_actions.capacity()*std::mem::size_of::<String>()+granted.allowed_actions.iter().map(String::capacity).sum::<usize>()+granted.lease_id.capacity()+granted.request_id.capacity();
                        match budget.reserve(charge) {Ok(memory)=>{
                            let fresh=lease.as_ref().is_none_or(|old|old.wire.lease_id!=granted.lease_id||old.wire.generation!=granted.generation);
                            if fresh {
                                grant_serial=match grant_serial.checked_add(1){Some(v)=>v,None=>break};
                                physical.clear();
                                if retire_editing_owner(&mut typing,&mut composition,&mut selected,&mut ui_state).is_err() {break;}
                                composition_deadline=None;ui_state.physical_notice=None;safety_stopped=false;
                            }
                            lease=Some(Lease{wire:granted,_memory:memory});status(&events,"Control granted.");if options.smoke_frames.is_some_and(|n|presented_frames>=n) {ui_state.clear(Lifecycle::Quit);break;}},Err(_)=>break}
                    },
                    Ok(ClientEvent::ControlRevoked(notice))=>{
                        if current_revocation(lease.as_ref(),&notice) {
                            physical.clear();
                            if retire_editing_owner(&mut typing,&mut composition,&mut selected,&mut ui_state).is_err() {break;}
                            composition_deadline=None;lease=None;
                            if let Some(observations)=&options.observations {observations.send_modify(|o|o.control_revoked=true);}
                            status(&events,"Observing; control released.");
                        }
                    },
                    Ok(ClientEvent::CommandResult(result))=>{
                        if physical.result(result.command_seq,&result.status).is_some_and(|control|Some(control)==ui_state.physical_control) {
                            safety_stopped=true;reconnect=true;
                            status(&events,"Physical dispatch rejected or unconfirmed; request fresh control after recovery.");break;
                        }
                        match composition.result(result.command_seq, &result.status, &result.source_commit, result.resulting_view_revision) {
                            Ok(true) => {
                                composition_deadline = if composition.pending_count() > 0 { Some(tokio::time::Instant::now() + Duration::from_secs(15)) } else { None };
                            }
                            Ok(false) => {}
                            Err(_) => {
                                composition.cancel(); ui_state.typing_notice=TypingNotice::Rejected;
                                safety_stopped=true; reconnect=true;
                                status(&events,"Composition rejected or unconfirmed; control stopped. Request fresh control after recovery.");
                                break;
                            }
                        }
                        if typing.result(result.command_seq,&result.status,&result.source_commit,result.resulting_view_revision).is_err() {
                            typing.cancel();composition.cancel();composition_deadline=None;selected=None;
                            ui_state.typing_notice=TypingNotice::Rejected;
                            ui_state.editing_generation=match ui_state.editing_generation.checked_add(1){Some(v)=>v,None=>break};
                            status(&events,"Remote text edit rejected or unconfirmed; pending input cancelled.");
                        } else {status(&events,if result.status=="Applied" || result.status=="APPLIED" {"Remote action applied."} else {"Remote action completed or rejected."});}
                    },
                Ok(ClientEvent::ResetRequired(_))=>{
                    pending=None;issued=None;issued_routing=None;presented=None;presented_routing=None;lease=None;selected=None;typing.cancel();composition.cancel();composition_deadline=None;
                    ui_state.clear(Lifecycle::Suspended);ui.send_replace(ui_state);
                    dirty=false;awaiting_resources=false;
                    status(&events,"Retained history expired; waiting for the coherent checkpoint.");
                },
                    Ok(ClientEvent::Error(_))=>status(&events,"Host rejected the request."),
                    Ok(_)=>{},
                    Err(error)=>{if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}status(&events,"Disconnected; input has been cleared.");reconnect=recoverable(&error);break;},
                },
                command=commands.recv()=>{
                    let Some(command)=command else {break;};
                    match command {
                        UiCommand::Quit=>{ui_state.clear(Lifecycle::Quit);break;},
                        UiCommand::Reconnect=>{reconnect=true;break;},
                        UiCommand::LoseResources=>{
                            if awaiting_resources {status(&events,"Wait for the current publication before dropping its cache.");continue;}
                            let old_physical=ui_state.input_control();
                            typing.cancel();composition.cancel();composition_deadline=None;selected=None;presented=None;presented_routing=None;issued=None;issued_routing=None;pending=None;
                            ui_state.clear(Lifecycle::Suspended);ui.send_replace(ui_state);
                            if let Err(error)=physical_io::guarded(client.resource_cache_lost(),old_physical,&mut priority,&mut safety_stopped).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}
                            awaiting_resources= !client.receiver().resources_ready().unwrap_or(false);dirty= !awaiting_resources;
                            status(&events,"Cache cleared; waiting for verified resources and a new frame.");
                        },
                        UiCommand::Acquire=>{let Some(request)=request_id(&mut requests) else {break;};if let Err(error)=physical_io::guarded(client.acquire_control(request),ui_state.input_control(),&mut priority,&mut safety_stopped).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}},
                        UiCommand::Release=>{
                            typing.cancel();composition.cancel();composition_deadline=None;selected=None;physical.clear();
                            if let Some(current)=lease.take() {
                                let Some(request)=request_id(&mut requests) else {break;};
                                if let Err(error)=physical_io::guarded(client.send(session::Kind::ReleaseControl,&session::ReleaseControl{request_id:request.to_vec(),lease_id:current.wire.lease_id,generation:current.wire.generation}),ui_state.input_control(),&mut priority,&mut safety_stopped).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}
                            }
                        },
                        UiCommand::Presented(stamp)=>{
                            if !ui_state.accepts(stamp) {continue;}
                            if !awaiting_resources && ui_state.accepts(stamp) && issued.is_some_and(|s:Stamp|s.serial==stamp.serial && s.revision==stamp.revision)
                                && client.receiver().state().is_ok_and(|s|s.revision==stamp.revision) {
                                if let Err(error)=physical_io::guarded(client.presented(stamp.revision),ui_state.input_control(),&mut priority,&mut safety_stopped).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}
                                presented=Some(stamp);presented_routing=issued_routing.clone();presented_frames+=1;status(&events,"Displayed.");
                                if automatic_control && !auto_acquired {auto_acquired=true;let Some(request)=request_id(&mut requests) else {break;};if let Err(error)=physical_io::guarded(client.acquire_control(request),ui_state.input_control(),&mut priority,&mut safety_stopped).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}}
                                if options.smoke_frames.is_some_and(|n|presented_frames>=n) && (!automatic_control || lease.is_some()) {ui_state.clear(Lifecycle::Quit);break;}
                            } else if !awaiting_resources {dirty=true;}
                        },
                        UiCommand::Composition { stamp, context, id, operation, _memory } => {
                            if !ui_state.accepts(stamp) { continue; }
                            let Some(binding) = binding else { continue; };
                            if !composition_cleanup::owns_binding(context,&binding) { continue; }
                            let cancel = matches!(&operation, rui_protocol_core::commands::CompositionOperation::Cancel);
                            if !ui_state.accepts(stamp) || !ui_state.wire_composition || (!cancel && (ui_state.editing != Some(context) || selected != Some(context.target))) {
                                safety_stopped=true; reconnect=true; ui_state.typing_notice=TypingNotice::Cancelled;
                                status(&events,"Composition owner changed; control stopped. Request fresh control after recovery."); break;
                            }
                            let state = match client.receiver().state() { Ok(state) => state, Err(_) => continue };
                            let stage = match &operation {
                                rui_protocol_core::commands::CompositionOperation::Begin(_) => NativeStage::Begin,
                                rui_protocol_core::commands::CompositionOperation::UpdatePreedit { text, selection } => NativeStage::Update { text, selection: *selection },
                                rui_protocol_core::commands::CompositionOperation::CommitText { text } => NativeStage::Commit(text),
                                rui_protocol_core::commands::CompositionOperation::Cancel => NativeStage::Cancel,
                            };
                            if composition.accept(context,id,stage,state,&binding).is_err() {
                                composition.cancel(); ui_state.typing_notice=TypingNotice::Limit;
                                safety_stopped=true; reconnect=true;
                                status(&events,"Composition could not be queued; control stopped. Request fresh control after recovery."); break;
                            }
                            ui_state.typing_notice=TypingNotice::None;
                        },
                        UiCommand::Physical(captured)=>{
                            if Some(captured.control)!=ui_state.physical_control {continue;}
                            let prepared=if ui_state.physical_ready {
                                match (binding,presented,presented_routing.as_deref(),device,client.receiver().state()) {
                                    (Some(binding),Some(stamp),Some(routing),Some(d),Ok(state)) if routing.revision()==stamp.revision =>captured.prepare_against_presented(state,binding,routing,ui_state.physical_control.unwrap(),physical.pointer_owner(),DeviceScope{seat:&d.seat_id,device:&d.device_id,key_namespace:&d.key_namespace,input_mode:&d.input_mode,text_owner:&d.text_owner}).ok(),
                                    _=>None,
                                }
                            } else {None};
                            let Some(prepared)=prepared else {safety_stopped=true;reconnect=true;status(&events,"Physical input route or readiness changed; request fresh control after recovery.");break;};
                            if physical.before_send(sequence,prepared.owner,&prepared.event).is_err() {safety_stopped=true;reconnect=true;status(&events,"Physical input limit reached; request fresh control after recovery.");break;}
                            if let Err(error)=physical_io::guarded(submit_wire(&mut client,CommandFamily::RawInput,&prepared.wire),ui_state.input_control(),&mut priority,&mut safety_stopped).await {
                                if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}
                                // An ambiguous physical send requires a manual new grant.
                                safety_stopped=true;reconnect=recoverable(&error);break;
                            }
                            sequence=client.commands().next_sequence();
                        },
                        UiCommand::Stroke{stamp,stroke}=>{
                            if !ui_state.accepts(stamp)||awaiting_resources||!presented.is_some_and(|p|p.serial==stamp.serial)||stroke.revision()!=stamp.revision||!stroke.matches_viewport(stamp.viewport) {status(&events,"Canvas view changed; the unsubmitted stroke was cancelled.");continue;}
                            let Some(binding)=binding else {status(&events,"Request raw input control before drawing.");continue;};
                            let Some(metadata)=opened.as_ref().filter(|m|m.input_devices.len()==1) else {status(&events,"No unambiguous source pointer device was granted.");continue;};
                            if !lease.as_ref().is_some_and(|l|l.wire.allowed_actions.iter().any(|a|a=="RawInputBatch")) {status(&events,"Source pointer input is not granted.");continue;}
                            let device=&metadata.input_devices[0];
                            let state=match client.receiver().state() {Ok(s)=>s,Err(_)=>continue};
                            let prepared=match stroke.into_command(state,binding,DeviceScope{seat:&device.seat_id,device:&device.device_id,key_namespace:&device.key_namespace,input_mode:&device.input_mode,text_owner:&device.text_owner}) {Ok(prepared)=>prepared,Err(_)=>{status(&events,"Canvas stroke cancelled: source route, geometry, or device changed.");continue;}};
                            if let Some(observations)=&options.observations {if let Some(events_digest)=prepared.event_digest() {observations.send_modify(|o|o.last_stroke=Some(StrokeObservation{command_sequence:sequence,event_count:prepared.wire().events.len(),events_digest}));}}
                            if let Err(error)=physical_io::guarded(submit_wire(&mut client,CommandFamily::RawInput,prepared.wire()),ui_state.input_control(),&mut priority,&mut safety_stopped).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}
                            sequence=client.commands().next_sequence();
                        },
                        UiCommand::ResizeSource{stamp,target,width,height}=>{
                            if !ui_state.accepts(stamp)||awaiting_resources||!presented.is_some_and(|p|p.serial==stamp.serial)||ui_state.source_resize!=Some(target) {status(&events,"Source resize context changed; wait for its latest frame.");continue;}
                            let Some(binding)=binding else {continue;};let state=match client.receiver().state(){Ok(s)=>s,Err(_)=>continue};
                            let prepared=match source_controls::resize_command(state,target,width,height,binding,&budget) {Ok(prepared)=>prepared,Err(_)=>{status(&events,"Source size or focused surface is unavailable.");continue;}};
                            if let Err(error)=physical_io::guarded(submit_wire(&mut client,CommandFamily::Resize,prepared.wire()),ui_state.input_control(),&mut priority,&mut safety_stopped).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}
                            sequence=client.commands().next_sequence();
                        },
                        UiCommand::ScrollSource{stamp,target,x,y}=>{
                            if !ui_state.accepts(stamp)||awaiting_resources||!presented.is_some_and(|p|p.serial==stamp.serial)||ui_state.source_scroll!=Some(target) {status(&events,"Select the latest remote scroll target first.");continue;}
                            let Some(binding)=binding else {continue;};let state=match client.receiver().state(){Ok(s)=>s,Err(_)=>continue};
                            let domain=match interaction::command(state,target,"ScrollTo",Arguments::Scroll{x,y},binding) {Ok(domain)=>domain,Err(_)=>{status(&events,"Remote scroll action or context is unavailable.");continue;}};
                            if let Err(error)=physical_io::guarded(submit_semantic(&mut client,domain),ui_state.input_control(),&mut priority,&mut safety_stopped).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}
                            sequence=client.commands().next_sequence();
                        },
                        input=>{
                            let stamp=match &input {UiCommand::Click{stamp,..}|UiCommand::Commit{stamp,..}|UiCommand::SelectAll{stamp}=>*stamp,_=>unreachable!()};
                            if !ui_state.accepts(stamp) {continue;}
                            if matches!(&input,UiCommand::Click{..}|UiCommand::SelectAll{..}) {
                                if composition.is_active() {
                                    composition.cancel();safety_stopped=true;reconnect=true;ui_state.typing_notice=TypingNotice::Cancelled;
                                    status(&events,"Composition cancelled before changing target. Request fresh control after recovery.");break;
                                }
                                composition.cancel();composition_deadline=None;
                                ui_state.typing_notice=if typing.cancel()>0{TypingNotice::Cancelled}else{TypingNotice::None};if matches!(&input,UiCommand::Click{..}) {selected=None;}ui_state.editing_generation=match ui_state.editing_generation.checked_add(1){Some(n)=>n,None=>break};}
                            if !ui_state.accepts(stamp) || presented.is_none() || (!matches!(&input,UiCommand::Commit{..}) && (awaiting_resources || !presented.is_some_and(|p|p.serial==stamp.serial) || !client.receiver().state().is_ok_and(|s|s.revision==stamp.revision))) {status(&events,"View changed; wait for the latest frame.");continue;}
                            let state=match client.receiver().state() {Ok(s)=>s,Err(_)=>continue};
                            let target=match input {
                                UiCommand::Click{point,..}=>{match interaction::hit_test(state,stamp.revision,stamp.viewport,point) {Ok(t)=>{selected=Some(t.semantic);t.semantic},Err(_)=>{selected=None;status(&events,if interaction::canvas_hit_test(state,stamp.revision,stamp.viewport,point).is_ok(){"Canvas stroke submits on release; every sample is retained."}else{"No enabled remote action here."});continue;}}},
                                UiCommand::Commit{context,..}=>context.target,
                                _=>match selected {Some(t)=>t,None=>{status(&events,"Select a remote text target first.");continue;}},
                            };
                            let Some(Record::Semantic(node))=state.records.get(&target) else {continue;};
                            let (action,args)=match input {
                                UiCommand::Click{..}=>{
                                    if node.actions.iter().any(|a|a.id=="Focus") && node.actions.iter().any(|a|a.id=="ReplaceText") {("Focus",Arguments::None)}
                                    else if node.actions.iter().any(|a|a.id=="Activate") {("Activate",Arguments::None)}
                                    else if node.actions.iter().any(|a|a.id=="SetValue") {
                                        let Some(TypedValue::Boolean(value))=&node.value else {status(&events,"This value requires an explicit editor.");continue;};("SetValue",Arguments::Checked(!value))
                                    } else {status(&events,if node.actions.iter().any(|a|a.id=="ScrollTo"){ "Remote scroll target selected; enter its absolute scroll position." }else{ "Text target selected; type to edit or Ctrl+A to select all." });continue;}
                                },
                                UiCommand::Commit{text,_memory,context,..}=>{
                                    if ui_state.wire_composition {
                                        safety_stopped=true;reconnect=true;ui_state.typing_notice=TypingNotice::Cancelled;
                                        status(&events,"Text ownership changed; control stopped. Request fresh control after recovery.");break;
                                    }
                                    if ui_state.editing != Some(context) || selected != Some(context.target) {ui_state.typing_notice=TypingNotice::Cancelled;status(&events,"Editing context changed; composition cancelled.");continue;}
                                    let Some(binding)=command_binding(&client,options.client_id,lease.as_ref(),sequence) else {continue;};
                                    if typing.accept(context,text,_memory,state,&binding).is_err() {typing.cancel();composition.cancel();composition_deadline=None;selected=None;ui_state.typing_notice=TypingNotice::Limit;status(&events,"Typing queue full or context changed; pending input cancelled.");}else{ui_state.typing_notice=TypingNotice::None;}
                                    continue;
                                },
                                UiCommand::SelectAll{..}=>{let Some(reference)=node.text else {continue;};let Some(Record::Text(model))=state.records.get(&reference.id) else {continue;};let Some(content)=&model.content else {continue;};("SetSelection",Arguments::Selection{anchor:0,focus:content.chars().count() as u64})},
                                _=>unreachable!(),
                            };
                            let Some(current)=&lease else {status(&events,"Request control to send remote input.");continue;};
                            if !current.wire.allowed_actions.iter().any(|a|a==action) {status(&events,"This action is not granted.");continue;}
                            let lease_id: [u8;16]=match current.wire.lease_id.as_slice().try_into() {Ok(id)=>id,Err(_)=>break};
                            let domain=match interaction::command(state,target,action,args,CommandBinding{epoch:client.epoch,view:client.view_id,client:options.client_id,lease:lease_id,generation:current.wire.generation,sequence}) {Ok(c)=>c,Err(_)=>{status(&events,"Remote target or editing context is unavailable.");continue;}};
                            if let Err(error)=physical_io::guarded(submit_semantic(&mut client,domain),ui_state.input_control(),&mut priority,&mut safety_stopped).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}sequence=client.commands().next_sequence();
                        },
                    }
                },
            }
        }
        if safety_stopped {
            ui_state.physical_notice =
                Some("Input control stopped. Request fresh control after recovery.");
            automatic_control = false;
        }
        physical.clear();
        if !reconnect {
            break 'connection;
        }
        pending = None;
        issued = None;
        issued_routing = None;
        presented = None;
        presented_routing = None;
        lease = None;
        selected = None;
        typing.cancel();
        composition.cancel();
        drop(opened.take());
        ui_state.clear(Lifecycle::Suspended);
        ui_state.typing_notice = TypingNotice::Cancelled;
        ui.send_replace(ui_state);
        if let Some(observations) = &options.observations {
            observations.send_modify(|o| {
                o.displayed = None;
                o.has_control = false;
                o.input_ready = false;
                o.physical_ready = false;
                o.editing_target = None;
                o.typing_pending = 0;
            });
        }
        let Some((reconnected, recovery_applied, recovery_opened)) = recover_connection(
            client,
            &options,
            &hello,
            &mut commands,
            &events,
            &mut shutdown,
        )
        .await
        else {
            ui_state.clear(Lifecycle::Closed);
            ui.send_replace(ui_state);
            if let Some(observations) = &options.observations {
                observations.send_modify(|o| {
                    o.closed = true;
                    o.displayed = None;
                    o.has_control = false;
                });
            }
            return;
        };
        client = reconnected;
        opened = recovery_opened;
        budget = client.receiver().memory_budget();
        sequence = client.commands().next_sequence();
        renderer = match Renderer::new_with_profile(
            RenderLimits::default(),
            budget.clone(),
            RenderProfile {
                cursor_vectors: client.profiles.iter().any(|p| p == CURSOR_PROFILE),
            },
        ) {
            Ok(renderer) => renderer,
            Err(_) => break 'connection,
        };
        _queue_memory = match budget.reserve(32 * std::mem::size_of::<UiCommand>() + 8192) {
            Ok(memory) => memory,
            Err(_) => break 'connection,
        };
        physical = match physical_continuity::Continuity::new(&budget) {
            Ok(value) => value,
            Err(()) => break 'connection,
        };
        typing = match crate::typing::TypingQueue::new_with_policy(
            budget.clone(),
            composition_policy(&client),
        ) {
            Ok(queue) => queue,
            Err(_) => break 'connection,
        };
        composition = match CompositionQueue::new(budget.clone(), composition_policy(&client)) {
            Ok(queue) => queue,
            Err(_) => break 'connection,
        };
        composition_deadline = None;
        pending = Some(UiEvent::Ready(budget.clone()));
        dirty = recovery_applied;
        awaiting_resources = false;
        auto_acquired = false;
        if let Some(observations) = &options.observations {
            observations.send_modify(|o| {
                o.binding = Some(ObservationBinding {
                    client: options.client_id,
                    epoch: client.epoch,
                    view: client.view_id,
                });
            });
        }
    }
    drop(pending);
    ui_state.clear(Lifecycle::Closed);
    ui.send_replace(ui_state);
    if let Some(observations) = &options.observations {
        observations.send_modify(|o| {
            o.closed = true;
            o.displayed = None;
            o.has_control = false;
            o.editing_target = None;
            o.typing_pending = 0;
        });
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), client.close()).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use rui_client_core::command_queue::CommandQueue;
    fn editing_fixture() -> State {
        let mut state = State {
            revision: 1,
            records: Default::default(),
            retired: Default::default(),
        };
        state.records.insert(
            20,
            Record::Semantic(SemanticNode {
                browser_input_type: None,
                id: 20,
                version: 1,
                role: SemanticRole::TextField,
                label: None,
                description: None,
                value: None,
                states: Default::default(),
                children: vec![],
                relations: vec![],
                actions: vec![Action {
                    id: "ReplaceText".into(),
                    schema_version: 1,
                }],
                text: Some(ObjectRef {
                    kind: ObjectKind::Text,
                    id: 40,
                }),
                collection: None,
                bindings: vec![],
                semantic_group: None,
                reading_order: None,
                sensitivity: Sensitivity::Public,
                completeness: Completeness::Full,
            }),
        );
        state.records.insert(
            40,
            Record::Text(TextModel {
                id: 40,
                version: 1,
                text_version: 1,
                status: TextStatus::Available,
                content: Some("Aé".into()),
                selection: Some(TextRange {
                    anchor: 2,
                    focus: 2,
                }),
                editable: true,
                selectable: true,
            }),
        );
        state.records.insert(
            50,
            Record::Session(SessionState {
                id: 50,
                version: 1,
                canvas: Rect {
                    x: 0.,
                    y: 0.,
                    width: 20.,
                    height: 20.,
                },
                cursor: None,
                focus: Some(ObjectRef {
                    kind: ObjectKind::Semantic,
                    id: 20,
                }),
                focus_surface: None,
                focus_generation: 1,
            }),
        );
        state
    }
    #[test]
    fn fresh_grant_retires_local_editing_but_preserves_exact_ambiguous_commands() {
        let budget = MemoryBudget::new(1 << 20);
        let state = editing_fixture();
        let old = CommandBinding {
            epoch: [1; 16],
            view: [2; 16],
            client: [3; 16],
            lease: [4; 16],
            generation: 1,
            sequence: 1,
        };
        let fresh = CommandBinding {
            lease: [9; 16],
            generation: 2,
            sequence: 2,
            ..old
        };
        let owner = interaction::editing_context(&state, 20, &old).unwrap();
        let mut typing = crate::typing::TypingQueue::new(budget.clone()).unwrap();
        typing
            .accept(
                owner,
                "x".into(),
                budget.reserve(2048).unwrap(),
                &state,
                &old,
            )
            .unwrap();
        assert!(typing.start(2, &state, &old).unwrap().is_some());
        let mut composition = CompositionQueue::new(budget.clone(), ClockPolicy::Strict).unwrap();
        composition
            .accept(owner, [1; 16], NativeStage::Begin, &state, &old)
            .unwrap();
        let prepared = composition
            .start(&state, &old, Some(state.revision))
            .unwrap()
            .unwrap();
        let wire = composition_wire(prepared.command);
        let mut exact = CommandQueue::new(old.epoch, budget.clone(), 4096).unwrap();
        exact
            .encode(
                CommandFamily::TextComposition,
                wire.encoded_len(),
                |bytes| wire.encode(bytes).map_err(|_| ()),
            )
            .unwrap();
        let retained = exact.latest().unwrap().bytes().to_vec();
        let mut selected = Some(owner.target);
        let mut ui = UiState {
            editing: Some(owner),
            editing_generation: 7,
            composition_control: Some(PhysicalControl {
                connection_generation: 1,
                grant_serial: 1,
            }),
            ..Default::default()
        };
        assert!(composition.is_active());
        retire_editing_owner(&mut typing, &mut composition, &mut selected, &mut ui).unwrap();
        assert_eq!(typing.context(), None);
        assert_eq!(typing.pending_count(), 0);
        assert_eq!(composition.context(), None);
        assert_eq!(composition.pending_count(), 0);
        assert!(!composition.is_active());
        assert_eq!(selected, None);
        assert_eq!(ui.editing, None);
        assert_eq!(ui.composition_control, None);
        assert_eq!(ui.editing_generation, 8);
        assert!(!composition.observe(&state, &fresh).unwrap());
        assert!(!composition_cleanup::owns_binding(owner, &fresh));
        assert_eq!(exact.latest().unwrap().bytes(), retained);
        assert_eq!(exact.next_sequence(), 2);
        // A late result may settle the retained exact request, but cannot
        // reinstall its retired local IM owner or invalidate the fresh grant.
        assert!(!composition.result(1, "Dispatched", "", None).unwrap());
        let fresh_owner = interaction::editing_context(&state, 20, &fresh).unwrap();
        assert!(composition_cleanup::owns_binding(fresh_owner, &fresh));
        composition
            .accept(fresh_owner, [2; 16], NativeStage::Begin, &state, &fresh)
            .unwrap();
        let fresh_command = composition
            .start(&state, &fresh, Some(state.revision))
            .unwrap()
            .unwrap();
        assert_eq!(fresh_command.command.header.control_lease_id, fresh.lease);
        assert_eq!(
            fresh_command.command.header.control_generation,
            fresh.generation
        );
        assert_eq!(exact.latest().unwrap().bytes(), retained);
        drop(fresh_command);
        drop(prepared.memory);
        drop(exact);
        drop(composition);
        drop(typing);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn delayed_revocation_cannot_cancel_a_new_grant() {
        let budget = MemoryBudget::new(4096);
        let lease = Lease {
            wire: session::ControlGranted {
                lease_id: vec![9; 16],
                generation: 2,
                ..Default::default()
            },
            _memory: budget.reserve(2048).unwrap(),
        };
        let mut notice = session::ControlRevoked {
            lease_id: vec![4; 16],
            generation: 1,
            reason: "old grant".into(),
        };
        assert!(!current_revocation(Some(&lease), &notice));
        notice.lease_id = lease.wire.lease_id.clone();
        assert!(!current_revocation(Some(&lease), &notice));
        notice.generation = lease.wire.generation;
        assert!(current_revocation(Some(&lease), &notice));
        assert!(!current_revocation(None, &notice));
    }
    #[test]
    fn lifecycle_watch_survives_full_frame_queue_and_is_terminal() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        assert!(tx.try_send(UiEvent::Status("occupied")).is_ok());
        let (state_tx, state_rx) = watch::channel(UiState::default());
        let old = Stamp {
            generation: 0,
            serial: 1,
            revision: 1,
            viewport: Viewport::new(1, 1),
            pixel_digest: [0; 32],
            records_digest: None,
        };
        let mut state = UiState::default();
        state.clear(Lifecycle::Closed);
        state_tx.send_replace(state);
        assert_eq!(state_rx.borrow().lifecycle, Lifecycle::Closed);
        assert!(!state_rx.borrow().accepts(old));
        assert!(!state.resume());
        assert!(matches!(rx.try_recv(), Ok(UiEvent::Status("occupied"))));
        let mut state = UiState::default();
        state.clear(Lifecycle::Quit);
        state.clear(Lifecycle::Closed);
        assert!(!state.resume());
        assert_eq!(state.lifecycle, Lifecycle::Quit);
    }
    #[test]
    fn resumed_render_rejects_every_queued_old_generation() {
        let old = Stamp {
            generation: 0,
            serial: 1,
            revision: 1,
            viewport: Viewport::new(1, 1),
            pixel_digest: [0; 32],
            records_digest: None,
        };
        let mut state = UiState::default();
        state.clear(Lifecycle::Suspended);
        assert!(!state.accepts(old));
        assert!(state.resume());
        assert!(!state.accepts(old));
        assert!(state.accepts(Stamp {
            generation: state.generation,
            ..old
        }));
    }
    #[test]
    fn acquisition_and_renewal_request_ids_are_fresh_until_checked_exhaustion() {
        let mut counter = 1;
        let first = request_id(&mut counter).unwrap();
        let second = request_id(&mut counter).unwrap();
        assert_ne!(first, second);
        assert_eq!(&first[8..], &1u64.to_be_bytes());
        let mut counter = u64::MAX;
        assert_eq!(request_id(&mut counter), None);
    }
}
