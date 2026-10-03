//! Optional GTK mirror shell. All document state remains in NetworkClient's
//! receiver; local viewport changes never generate application commands.
use crate::{
    command_queue::CommandQueue,
    interaction::{self, CommandBinding, EditingContext},
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

pub mod gtk;
pub mod native;

pub struct Options {
    pub address: SocketAddr,
    pub server_name: String,
    pub certificate: Vec<u8>,
    pub credential: Credential,
    pub client_id: [u8; 16],
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
    pub control_revoked: bool,
    pub editing_target: Option<u64>,
    pub typing_pending: usize,
    pub typing_notice: TypingNotice,
    pub notice_visible: bool,
    pub closed: bool,
    pub local_controls: Option<LocalControls>,
    pub last_stroke: Option<StrokeObservation>,
    pub stroke_notice: Option<&'static str>,
    pub stroke_notice_visible: bool,
    pub last_action: Option<ActionObservation>,
    pub failure_class: Option<&'static str>,
    pub render_timing: RenderTimingObservation,
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
    SelectAll {
        stamp: Stamp,
    },
    Stroke {
        stamp: Stamp,
        stroke: PointerStroke,
    },
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
    Frame { stamp: Stamp, pixels: RgbaFrame },
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
    pub raw_input: bool,
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
            raw_input: false,
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
        self.raw_input = false;
        self.source_resize = None;
        self.source_scroll = None;
    }
    pub fn resume(&mut self) -> bool {
        if self.lifecycle == Lifecycle::Suspended {
            self.lifecycle = Lifecycle::Live;
        }
        self.lifecycle == Lifecycle::Live
    }
    pub fn accepts(&self, stamp: Stamp) -> bool {
        self.lifecycle == Lifecycle::Live && self.generation == stamp.generation
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
    pending: &mut CommandQueue,
) -> Result<(), TransportError> {
    if domain.command_seq != pending.next_sequence() {
        return Err(TransportError::Protocol);
    }
    let wire = wire_command(domain);
    submit_wire(client, CommandFamily::Semantic, &wire, pending).await
}
async fn submit_wire<T: Message>(
    client: &mut NetworkClient,
    family: CommandFamily,
    wire: &T,
    pending: &mut CommandQueue,
) -> Result<(), TransportError> {
    let length = wire.encoded_len();
    pending
        .encode(family, length, |bytes| wire.encode(bytes).map_err(|_| ()))
        .map_err(|_| TransportError::Limit)?;
    client
        .submit_family(family, pending.latest().unwrap().bytes())
        .await
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
    pending: &mut CommandQueue,
    commands: &mut mpsc::Receiver<UiCommand>,
    events: &SyncSender<UiEvent>,
    shutdown: &mut watch::Receiver<bool>,
) -> Option<(NetworkClient, bool, Option<OpenedView>)> {
    let mut context = (client.epoch, client.view_id);
    let mut retained = client.into_retained();
    loop {
        let attempt = tokio::select! {
            _=shutdown.changed()=>return None,
            result=retained.reconnect(options.address,options.server_name.clone(),options.certificate.clone(),&options.credential,hello.clone())=>result,
        };
        if let Ok(mut client) = attempt {
            let binding = (client.epoch, client.view_id);
            let ready = binding == context
                || pending
                    .rebind(
                        client.epoch,
                        client.receiver().memory_budget(),
                        client.limits.envelope_bytes.saturating_sub(128),
                    )
                    .is_ok();
            if ready {
                context = binding;
                let mut applied = false;
                let mut opened = None;
                let recovery = async {
                    client.open_view().await?;
                    // Old-lease exact retries either recover a retained result
                    // or consume a rejection. They cannot restore source input.
                    while !pending.is_empty() {
                        let command = pending.commands().next().unwrap();
                        let sequence = command.sequence();
                        client
                            .submit_family(command.family(), command.bytes())
                            .await?;
                        for index in 0..1024 {
                            let result = client.receive().await?;
                            match result {
                                ClientEvent::CommandResult(r) if r.command_seq == sequence => break,
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
                        pending.complete(sequence);
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
            } else {
                status(
                    events,
                    "New limits cannot recover pending commands; input remains paused.",
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
fn wire_command(c: rui_protocol_core::commands::SemanticCommand) -> core::SemanticCommand {
    let arguments = match c.arguments {
        Arguments::None => core::semantic_command::Arguments::NoArguments(core::EmptyArguments {}),
        Arguments::Checked(checked) => {
            core::semantic_command::Arguments::SetValue(core::ValueArguments {
                value: Some(core::value_arguments::Value::Checked(checked)),
            })
        }
        Arguments::Number(number) => {
            core::semantic_command::Arguments::SetValue(core::ValueArguments {
                value: Some(core::value_arguments::Value::Number(number)),
            })
        }
        Arguments::ValueText(text) => {
            core::semantic_command::Arguments::SetValue(core::ValueArguments {
                value: Some(core::value_arguments::Value::Text(text)),
            })
        }
        Arguments::ReplaceText { start, end, text } => {
            core::semantic_command::Arguments::ReplaceText(core::TextArguments { start, end, text })
        }
        Arguments::Selection { anchor, focus } => {
            core::semantic_command::Arguments::SetSelection(core::SelectionArguments {
                anchor,
                focus,
            })
        }
        Arguments::Scroll { x, y } => {
            core::semantic_command::Arguments::ScrollTo(core::ScrollArguments { x, y })
        }
    };
    core::SemanticCommand {
        epoch: c.epoch.to_vec(),
        view_id: c.view_id.to_vec(),
        client_instance_id: c.client_instance_id.to_vec(),
        command_seq: c.command_seq,
        control_lease_id: c.control_lease_id.to_vec(),
        control_generation: c.control_generation,
        target_id: c.target_id,
        action_id: c.action_id,
        action_schema_version: c.action_schema_version,
        observed_view_revision: c.observed_view_revision,
        preconditions: Some(core::Preconditions {
            target_version: c.preconditions.target_version,
            text_version: c.preconditions.text_version,
            model_version: c.preconditions.model_version,
            focus_generation: c.preconditions.focus_generation,
        }),
        arguments: Some(arguments),
    }
}
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
    mut commands: mpsc::Receiver<UiCommand>,
    mut viewport: watch::Receiver<Viewport>,
    events: SyncSender<UiEvent>,
    ui: watch::Sender<UiState>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut ui_state = UiState::default();
    let mut profiles = SessionConfig::default().supported_profiles;
    profiles.insert("glyph.outline-single.v1".into());
    profiles.insert(CURSOR_PROFILE.into());
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
        Err(_) => {
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
    let mut initial_reconnect = match client.open_view().await {
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
    let mut presented: Option<Stamp> = None;
    let mut lease: Option<Lease> = None;
    let mut sequence = 1u64;
    let mut selected = None;
    let mut auto_acquired = false;
    let mut presented_frames = 0u64;
    let mut requests = 1u64;
    let mut awaiting_resources = false;
    let mut render_timing = RenderTimingObservation::default();
    let mut opened: Option<OpenedView> = None;
    let mut typing = match crate::typing::TypingQueue::new(budget.clone()) {
        Ok(queue) => queue,
        Err(_) => {
            ui_state.clear(Lifecycle::Closed);
            ui.send_replace(ui_state);
            return;
        }
    };
    let mut pending_commands = match CommandQueue::new(
        client.epoch,
        budget.clone(),
        client.limits.envelope_bytes.saturating_sub(128),
    ) {
        Ok(queue) => queue,
        Err(_) => {
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
                            .any(|a| a == "ReplaceText") =>
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
            ui_state.editing = if selected.is_some() {
                typing.context().or(raw_editing)
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
            ui_state.raw_input = granted("RawInputBatch")
                && input_ready
                && opened.as_ref().is_some_and(|v| v.input_devices.len() == 1);
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
                    o.typing_pending = typing.pending_count();
                    o.has_control = lease.is_some();
                    o.input_ready = input_ready;
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
                                    selected = None;
                                    ui_state.typing_notice = TypingNotice::Cancelled;
                                    status(&events, "Pending typing cancelled before dispatch.");
                                    continue;
                                }
                            };
                            if let Err(error) =
                                submit_semantic(&mut client, domain, &mut pending_commands).await
                            {
                                reconnect = recoverable(&error);
                                break;
                            }
                            sequence = pending_commands.next_sequence();
                            drop(prepared.memory);
                        }
                        Ok(None) => {}
                        Err(_) => {
                            typing.cancel();
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
                _=shutdown.changed()=>{ui_state.clear(Lifecycle::Quit);break;},
                scheduled=tick.tick()=>{
                    render_timing.accepted_ticks=render_timing.accepted_ticks.saturating_add(1);
                    let lateness=tokio::time::Instant::now().saturating_duration_since(scheduled).as_nanos();
                    if lateness>16_666_667 {render_timing.late_ticks=render_timing.late_ticks.saturating_add(1);render_timing.elapsed_missed_periods=render_timing.elapsed_missed_periods.saturating_add((lateness/16_666_667).min(u128::from(u64::MAX)) as u64);}
                    if dirty && !awaiting_resources && pending.is_none() && client.receiver().resources_ready().unwrap_or(false) {
                        let work_start=std::time::Instant::now();
                        let v=*viewport.borrow();
                        match client.receiver().state().map_err(|_|()).and_then(|state|renderer.render_view(state,v,client.receiver()).map_err(|_|())) {
                            Ok(frame)=>match frame.into_rgba8_over(Color{red:1.0,green:1.0,blue:1.0,alpha:1.0}) {
                                Ok(pixels)=>{if !ui_state.resume() {break;}ui.send_replace(ui_state);serial=match serial.checked_add(1) {Some(v)=>v,None=>break};let pixel_digest=if options.observations.is_some(){Sha256::digest(pixels.as_ref()).into()}else{[0;32]};let records_digest=if options.observations.is_some(){client.receiver().state().ok().and_then(records_digest)}else{None};let stamp=Stamp{generation:ui_state.generation,serial,revision:pixels.revision,viewport:pixels.viewport,pixel_digest,records_digest};issued=Some(stamp);pending=Some(UiEvent::Frame{stamp,pixels});dirty=false;},
                                Err(_)=>{pending=None;ui_state.clear(Lifecycle::Suspended);status(&events,"Display allocation limit reached.");dirty=false;},
                            },
                            Err(_)=>{pending=None;ui_state.clear(Lifecycle::Suspended);status(&events,"Scene cannot be rendered within this profile and its limits.");dirty=false;},
                        }
                        let elapsed=work_start.elapsed();render_timing.rendered_frames=render_timing.rendered_frames.saturating_add(1);
                        render_timing.max_work_us=render_timing.max_work_us.max(elapsed.as_micros().min(u128::from(u64::MAX)) as u64);
                        if elapsed.as_nanos()>16_666_667 {render_timing.work_over_budget=render_timing.work_over_budget.saturating_add(1);}
                    }
                    if let Some(observations)=&options.observations {observations.send_modify(|o|o.render_timing=render_timing);}
                },
                _=ping.tick()=>{
                    if let Err(error)=client.send(session::Kind::Ping,&session::Ping{nonce:serial}).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}
                    if let Some(current)=&lease {
                        let Some(request)=request_id(&mut requests) else {break;};
                        if let Err(error)=client.send(session::Kind::RenewControl,&session::RenewControl{request_id:request.to_vec(),lease_id:current.wire.lease_id.clone(),generation:current.wire.generation}).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}
                    }
                },
                changed=viewport.changed()=>{if changed.is_err() {break;}dirty=true;},
                event=client.receive()=>match event {
                    Ok(ClientEvent::ViewOpened(metadata))=>{if opened.is_some() {break;}opened=Some(metadata);},
                    Ok(ClientEvent::Applied{..})=>{awaiting_resources=false;dirty=true;status(&events,"View applied; waiting for display.");},
                    Ok(ClientEvent::ResourcesNeeded{..})=>{awaiting_resources=true;status(&events,"Waiting for verified resources; typing remains queued.");},
                    Ok(ClientEvent::ControlGranted(granted))=>{
                        if let Some(observations)=&options.observations {observations.send_modify(|o|o.control_revoked=false);}
                        let charge=2048+granted.allowed_actions.capacity()*std::mem::size_of::<String>()+granted.allowed_actions.iter().map(String::capacity).sum::<usize>()+granted.lease_id.capacity()+granted.request_id.capacity();
                        match budget.reserve(charge) {Ok(memory)=>{lease=Some(Lease{wire:granted,_memory:memory});status(&events,"Control granted.");if options.smoke_frames.is_some_and(|n|presented_frames>=n) {ui_state.clear(Lifecycle::Quit);break;}},Err(_)=>break}
                    },
                    Ok(ClientEvent::ControlRevoked(_))=>{lease=None;selected=None;if let Some(observations)=&options.observations {observations.send_modify(|o|o.control_revoked=true);}status(&events,"Observing; control released.");},
                    Ok(ClientEvent::CommandResult(result))=>{
                        pending_commands.complete(result.command_seq);
                        if typing.result(result.command_seq,&result.status,&result.source_commit,result.resulting_view_revision).is_err() {
                            typing.cancel();selected=None;
                            ui_state.typing_notice=TypingNotice::Rejected;
                            ui_state.editing_generation=match ui_state.editing_generation.checked_add(1){Some(v)=>v,None=>break};
                            status(&events,"Remote text edit rejected or unconfirmed; pending input cancelled.");
                        } else {status(&events,if result.status=="Applied" || result.status=="APPLIED" {"Remote action applied."} else {"Remote action completed or rejected."});}
                    },
                Ok(ClientEvent::ResetRequired(_))=>{
                    pending=None;issued=None;presented=None;lease=None;selected=None;typing.cancel();
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
                            typing.cancel();selected=None;presented=None;issued=None;pending=None;
                            ui_state.clear(Lifecycle::Suspended);ui.send_replace(ui_state);
                            if let Err(error)=client.resource_cache_lost().await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}
                            awaiting_resources= !client.receiver().resources_ready().unwrap_or(false);dirty= !awaiting_resources;
                            status(&events,"Cache cleared; waiting for verified resources and a new frame.");
                        },
                        UiCommand::Acquire=>{let Some(request)=request_id(&mut requests) else {break;};if let Err(error)=client.acquire_control(request).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}},
                        UiCommand::Release=>{
                            typing.cancel();selected=None;
                            if let Some(current)=lease.take() {
                                let Some(request)=request_id(&mut requests) else {break;};
                                if let Err(error)=client.send(session::Kind::ReleaseControl,&session::ReleaseControl{request_id:request.to_vec(),lease_id:current.wire.lease_id,generation:current.wire.generation}).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}
                            }
                        },
                        UiCommand::Presented(stamp)=>{
                            if !ui_state.accepts(stamp) {continue;}
                            if !awaiting_resources && ui_state.accepts(stamp) && issued.is_some_and(|s:Stamp|s.serial==stamp.serial && s.revision==stamp.revision)
                                && client.receiver().state().is_ok_and(|s|s.revision==stamp.revision) {
                                if let Err(error)=client.presented(stamp.revision).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}
                                presented=Some(stamp);presented_frames+=1;status(&events,"Displayed.");
                                if options.auto_control && !auto_acquired {auto_acquired=true;let Some(request)=request_id(&mut requests) else {break;};if let Err(error)=client.acquire_control(request).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}}
                                if options.smoke_frames.is_some_and(|n|presented_frames>=n) && (!options.auto_control || lease.is_some()) {ui_state.clear(Lifecycle::Quit);break;}
                            } else if !awaiting_resources {dirty=true;}
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
                            if let Err(error)=submit_wire(&mut client,CommandFamily::RawInput,prepared.wire(),&mut pending_commands).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}
                            sequence=pending_commands.next_sequence();
                        },
                        UiCommand::ResizeSource{stamp,target,width,height}=>{
                            if !ui_state.accepts(stamp)||awaiting_resources||!presented.is_some_and(|p|p.serial==stamp.serial)||ui_state.source_resize!=Some(target) {status(&events,"Source resize context changed; wait for its latest frame.");continue;}
                            let Some(binding)=binding else {continue;};let state=match client.receiver().state(){Ok(s)=>s,Err(_)=>continue};
                            let prepared=match source_controls::resize_command(state,target,width,height,binding,&budget) {Ok(prepared)=>prepared,Err(_)=>{status(&events,"Source size or focused surface is unavailable.");continue;}};
                            if let Err(error)=submit_wire(&mut client,CommandFamily::Resize,prepared.wire(),&mut pending_commands).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}
                            sequence=pending_commands.next_sequence();
                        },
                        UiCommand::ScrollSource{stamp,target,x,y}=>{
                            if !ui_state.accepts(stamp)||awaiting_resources||!presented.is_some_and(|p|p.serial==stamp.serial)||ui_state.source_scroll!=Some(target) {status(&events,"Select the latest remote scroll target first.");continue;}
                            let Some(binding)=binding else {continue;};let state=match client.receiver().state(){Ok(s)=>s,Err(_)=>continue};
                            let domain=match interaction::command(state,target,"ScrollTo",Arguments::Scroll{x,y},binding) {Ok(domain)=>domain,Err(_)=>{status(&events,"Remote scroll action or context is unavailable.");continue;}};
                            if let Err(error)=submit_semantic(&mut client,domain,&mut pending_commands).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}
                            sequence=pending_commands.next_sequence();
                        },
                        input=>{
                            let stamp=match &input {UiCommand::Click{stamp,..}|UiCommand::Commit{stamp,..}|UiCommand::SelectAll{stamp}=>*stamp,_=>unreachable!()};
                            if !ui_state.accepts(stamp) {continue;}
                            if matches!(&input,UiCommand::Click{..}|UiCommand::SelectAll{..}) {ui_state.typing_notice=if typing.cancel()>0{TypingNotice::Cancelled}else{TypingNotice::None};if matches!(&input,UiCommand::Click{..}) {selected=None;}ui_state.editing_generation=match ui_state.editing_generation.checked_add(1){Some(n)=>n,None=>break};}
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
                                    if ui_state.editing != Some(context) || selected != Some(context.target) {ui_state.typing_notice=TypingNotice::Cancelled;status(&events,"Editing context changed; composition cancelled.");continue;}
                                    let Some(binding)=command_binding(&client,options.client_id,lease.as_ref(),sequence) else {continue;};
                                    if typing.accept(context,text,_memory,state,&binding).is_err() {typing.cancel();selected=None;ui_state.typing_notice=TypingNotice::Limit;status(&events,"Typing queue full or context changed; pending input cancelled.");}else{ui_state.typing_notice=TypingNotice::None;}
                                    continue;
                                },
                                UiCommand::SelectAll{..}=>{let Some(reference)=node.text else {continue;};let Some(Record::Text(model))=state.records.get(&reference.id) else {continue;};let Some(content)=&model.content else {continue;};("SetSelection",Arguments::Selection{anchor:0,focus:content.chars().count() as u64})},
                                _=>unreachable!(),
                            };
                            let Some(current)=&lease else {status(&events,"Request control to send remote input.");continue;};
                            if !current.wire.allowed_actions.iter().any(|a|a==action) {status(&events,"This action is not granted.");continue;}
                            let lease_id: [u8;16]=match current.wire.lease_id.as_slice().try_into() {Ok(id)=>id,Err(_)=>break};
                            let domain=match interaction::command(state,target,action,args,CommandBinding{epoch:client.epoch,view:client.view_id,client:options.client_id,lease:lease_id,generation:current.wire.generation,sequence}) {Ok(c)=>c,Err(_)=>{status(&events,"Remote target or editing context is unavailable.");continue;}};
                            if let Err(error)=submit_semantic(&mut client,domain,&mut pending_commands).await {if let Some(observations)=&options.observations {observations.send_modify(|o|o.failure_class=Some(error_class(&error)));}reconnect=recoverable(&error);break;}sequence=pending_commands.next_sequence();
                        },
                    }
                },
            }
        }
        if !reconnect {
            break 'connection;
        }
        pending = None;
        issued = None;
        presented = None;
        lease = None;
        selected = None;
        typing.cancel();
        drop(opened.take());
        ui_state.clear(Lifecycle::Suspended);
        ui_state.typing_notice = TypingNotice::Cancelled;
        ui.send_replace(ui_state);
        if let Some(observations) = &options.observations {
            observations.send_modify(|o| {
                o.displayed = None;
                o.has_control = false;
                o.input_ready = false;
                o.editing_target = None;
                o.typing_pending = 0;
            });
        }
        let Some((reconnected, recovery_applied, recovery_opened)) = recover_connection(
            client,
            &options,
            &hello,
            &mut pending_commands,
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
        sequence = pending_commands.next_sequence();
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
        typing = match crate::typing::TypingQueue::new(budget.clone()) {
            Ok(queue) => queue,
            Err(_) => break 'connection,
        };
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
    let _ = client.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
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
