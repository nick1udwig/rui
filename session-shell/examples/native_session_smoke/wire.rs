//! Test-only third receiver and real GTK paint witness for wire authority negatives.
//! No borrowed view marker, synthetic Presented, native receipt, or second ledger.
use super::*;
use gtk4::{gdk, subclass::prelude::*};
use prost::Message;
use rui_client_desktop::RgbaFrame;
use rui_protocol_core::{
    commands::CommandFamily,
    model::{State, Surface},
    resources::MemoryReservation,
    session::{ClientHello, SessionConfig},
    wire::proto as pb,
};
use rui_transport::{
    pairing::Credential, proto as p, ClientEvent, NetworkClient, OpenedView, TransportError,
};
use std::{
    cell::Cell,
    net::SocketAddr,
    sync::mpsc::{Receiver, SyncSender},
};

const PROBE: &str = "RUI native wire paint probe";
const CLIENT: [u8; 16] = [83; 16];
#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    epoch: [u8; 16],
    view: [u8; 16],
    revision: u64,
    serial: u64,
    pixels: [u8; 32],
}
struct Paint {
    stamp: Stamp,
    rgba: RgbaFrame,
    budget: MemoryBudget,
}
enum UiRequest {
    Frame(Paint),
    Clear,
}
#[derive(Clone)]
pub struct PaintPort {
    frames: SyncSender<UiRequest>,
    painted: watch::Receiver<Option<Stamp>>,
}
pub struct PaintUi {
    frames: Receiver<UiRequest>,
    painted: watch::Sender<Option<Stamp>>,
}
pub fn channel() -> (PaintPort, PaintUi) {
    let (frames, receiver) = mpsc::sync_channel(1);
    let (painted, watch) = watch::channel(None);
    (
        PaintPort {
            frames,
            painted: watch,
        },
        PaintUi {
            frames: receiver,
            painted,
        },
    )
}
mod imp {
    use super::*;
    #[derive(Default)]
    pub struct Canvas {
        pub(super) texture: RefCell<Option<gdk::MemoryTexture>>,
        pub(super) stamp: Cell<Option<Stamp>>,
        pub(super) submitted: Cell<Option<Stamp>>,
    }
    #[glib::object_subclass]
    impl ObjectSubclass for Canvas {
        const NAME: &'static str = "RuiNativeWirePaintCanvas";
        type Type = super::Canvas;
        type ParentType = gtk4::Widget;
    }
    impl ObjectImpl for Canvas {}
    impl WidgetImpl for Canvas {
        fn measure(&self, orientation: gtk4::Orientation, _: i32) -> (i32, i32, i32, i32) {
            let size = if orientation == gtk4::Orientation::Horizontal {
                200
            } else {
                120
            };
            (size, size, -1, -1)
        }
        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            if let (Some(texture), Some(stamp)) = (self.texture.borrow().as_ref(), self.stamp.get())
            {
                snapshot.append_texture(texture, &graphene::Rect::new(0., 0., 200., 120.));
                self.submitted.set(Some(stamp));
            }
        }
    }
}
glib::wrapper! { pub struct Canvas(ObjectSubclass<imp::Canvas>) @extends gtk4::Widget, @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget; }
struct Window {
    window: gtk4::ApplicationWindow,
    canvas: Canvas,
    clock: Rc<RefCell<Option<(gdk::FrameClock, glib::SignalHandlerId)>>>,
    _native: Rc<MemoryReservation>,
}
impl Window {
    fn new(budget: &MemoryBudget, painted: watch::Sender<Option<Stamp>>) -> Result<Self, Error> {
        // Explicit opaque GTK/GDK allowance on the same receiver budget, plus
        // the already charged RgbaFrame/native-copy lifetime. Queue capacity1.
        let native = Rc::new(budget.reserve(4 << 20)?);
        let app = gio::Application::default()
            .ok_or("wire paint application missing")?
            .downcast::<gtk4::Application>()
            .map_err(|_| "wire paint application type")?;
        let canvas: Canvas = glib::Object::builder().build();
        let window = gtk4::ApplicationWindow::builder()
            .application(&app)
            .title(PROBE)
            .default_width(200)
            .default_height(120)
            .child(&canvas)
            .build();
        let clock = Rc::new(RefCell::new(None));
        let hook = clock.clone();
        let weak = canvas.downgrade();
        window.connect_realize(move |window| {
            if let Some(frame_clock) = window.frame_clock() {
                let weak = weak.clone();
                let painted = painted.clone();
                let handler = frame_clock.connect_after_paint(move |_| {
                    if let Some(canvas) = weak.upgrade() {
                        if let Some(stamp) = canvas.imp().submitted.get() {
                            painted.send_replace(Some(stamp));
                        }
                    }
                });
                hook.replace(Some((frame_clock, handler)));
            }
        });
        // Charge survives escaped native clones, not merely this wrapper.
        let hold = native.clone();
        window.connect_destroy(move |_| {
            let _keep = &hold;
        });
        window.present();
        Ok(Self {
            window,
            canvas,
            clock,
            _native: native,
        })
    }
    fn frame(&self, frame: Paint) {
        let bytes = glib::Bytes::from_owned(frame.rgba);
        let texture = gdk::MemoryTexture::new(200, 120, gdk::MemoryFormat::R8g8b8a8, &bytes, 800);
        self.canvas.imp().texture.replace(Some(texture));
        self.canvas.imp().submitted.set(None);
        self.canvas.imp().stamp.set(Some(frame.stamp));
        self.canvas.queue_draw();
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        if let Some((clock, handler)) = self.clock.borrow_mut().take() {
            clock.disconnect(handler);
        }
        self.canvas.imp().texture.replace(None);
        self.canvas.imp().stamp.set(None);
        self.canvas.imp().submitted.set(None);
        self.window.set_child(None::<&gtk4::Widget>);
        self.window.destroy();
    }
}
/// GTK main-thread only. The finite driver sends one charged frame at a time.
pub fn install(ui: PaintUi, stop: watch::Receiver<bool>) {
    let mut window = None::<Window>;
    glib::timeout_add_local(Duration::from_millis(16), move || {
        if *stop.borrow() {
            window.take();
            return glib::ControlFlow::Break;
        }
        match ui.frames.try_recv() {
            Ok(UiRequest::Frame(frame)) => {
                if window.is_none() {
                    match Window::new(&frame.budget, ui.painted.clone()) {
                        Ok(created) => window = Some(created),
                        Err(_) => {
                            ui.painted.send_replace(None);
                            return glib::ControlFlow::Break;
                        }
                    }
                }
                window.as_ref().unwrap().frame(frame);
            }
            Ok(UiRequest::Clear) => {
                window.take();
                ui.painted.send_replace(None);
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                window.take();
                return glib::ControlFlow::Break;
            }
        }
        glib::ControlFlow::Continue
    });
}
fn hello() -> ClientHello {
    let config = SessionConfig::default();
    let mut supported_profiles = config.supported_profiles;
    supported_profiles.insert("cursor.vector.v1".into());
    ClientHello {
        client_instance_id: CLIENT,
        supported_versions: [1].into_iter().collect(),
        supported_profiles,
        required_profiles: config.required_profiles,
        receiver_limits: Default::default(),
        resume: None,
    }
}
async fn connect(address: SocketAddr, credential: &Credential) -> Result<NetworkClient, Error> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match NetworkClient::connect(
            address,
            "localhost".into(),
            include_bytes!("../../../transport/tests/data/localhost.der").to_vec(),
            credential,
            hello(),
        )
        .await
        {
            Err(TransportError::ProducerBusy) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            result => return Ok(result?),
        }
    }
}
async fn event(client: &mut NetworkClient) -> Result<ClientEvent, Error> {
    Ok(tokio::time::timeout(Duration::from_secs(5), client.receive()).await??)
}
fn traffic(event: ClientEvent) -> Result<(), Error> {
    match event {
        ClientEvent::Applied { .. }
        | ClientEvent::ResourcesNeeded { .. }
        | ClientEvent::ResourceAvailable
        | ClientEvent::ControlRevoked(_) => Ok(()),
        _ => Err("unexpected packet in wire proof phase".into()),
    }
}
async fn ready(client: &mut NetworkClient) -> Result<OpenedView, Error> {
    let mut opened = None;
    for _ in 0..256 {
        match event(client).await? {
            ClientEvent::ViewOpened(view) if opened.is_none() => opened = Some(view),
            ClientEvent::Applied { revision } if revision > 0 => {
                return opened.ok_or("Applied before Opened".into())
            }
            ClientEvent::ResourcesNeeded { .. } | ClientEvent::ResourceAvailable => {}
            _ => return Err("unexpected initial wire publication event".into()),
        }
    }
    Err("initial wire publication event count ceiling".into())
}
async fn paint(
    client: &mut NetworkClient,
    ui: &PaintPort,
    port: &NativeProducerPort,
    serial: u64,
) -> Result<(), Error> {
    if !client.receiver().resources_ready()? {
        return Err("wire paint resources unavailable".into());
    }
    let state = client.receiver().state()?;
    let source = port.projected_source(CLIENT, client.view_id).await?;
    if state.revision == 0
        || desktop::records_digest(state) != desktop::records_digest(source.state())
    {
        return Err("wire own validated state differs from its authoritative projection".into());
    }
    let budget = client.receiver().memory_budget();
    let renderer = Renderer::new_with_profile(
        RenderLimits::default(),
        budget.clone(),
        RenderProfile {
            cursor_vectors: true,
        },
    )?;
    let viewport = Viewport {
        width: 200,
        height: 120,
        origin: Point { x: 0., y: 0. },
        scale: 1.,
    };
    let rgba = renderer
        .render_view(state, viewport, client.receiver())?
        .into_rgba8_over(white())?;
    let stamp = Stamp {
        epoch: client.epoch,
        view: client.view_id,
        revision: state.revision,
        serial,
        pixels: Sha256::digest(rgba.as_ref()).into(),
    };
    ui.frames
        .try_send(UiRequest::Frame(Paint {
            stamp,
            rgba,
            budget,
        }))
        .map_err(|_| "bounded wire paint slot unavailable")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while *ui.painted.borrow() != Some(stamp) {
        if Instant::now() >= deadline {
            return Err("actual third GTK snapshot/after-paint deadline".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    client.presented(stamp.revision).await?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while port.presented_revision(CLIENT, client.view_id).await? != Some(stamp.revision) {
        if Instant::now() >= deadline {
            return Err("third actual paint acknowledgement not recorded".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(())
}
async fn sync_state(client: &mut NetworkClient, port: &NativeProducerPort) -> Result<(), Error> {
    for _ in 0..256 {
        let current = port.projected_source(CLIENT, client.view_id).await?;
        if desktop::records_digest(client.receiver().state()?)
            == desktop::records_digest(current.state())
            && client.receiver().resources_ready()?
        {
            return Ok(());
        }
        traffic(event(client).await?)?;
    }
    Err("wire state convergence packet ceiling".into())
}
async fn acquire(client: &mut NetworkClient, id: u8) -> Result<p::ControlGranted, Error> {
    client.acquire_control([id; 16]).await?;
    for _ in 0..256 {
        match event(client).await? {
            ClientEvent::ControlGranted(lease) => {
                if lease.request_id != [id; 16]
                    || lease.generation == 0
                    || lease.lease_id.len() != 16
                {
                    return Err("wire control grant does not match explicit request".into());
                }
                return Ok(lease);
            }
            other => traffic(other)?,
        }
    }
    Err("wire control reply packet ceiling".into())
}
async fn release(
    client: &mut NetworkClient,
    lease: &p::ControlGranted,
    request: u8,
    host: &HostPort,
) -> Result<(), Error> {
    client
        .send(
            p::Kind::ReleaseControl,
            &p::ReleaseControl {
                request_id: vec![request; 16],
                lease_id: lease.lease_id.clone(),
                generation: lease.generation,
            },
        )
        .await?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let d = diagnostics(host).await?;
        if d.lease.is_none() && owner_ready(host).await? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("wire release/actual native STOP deadline".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
async fn owner_ready(host: &HostPort) -> Result<bool, Error> {
    let pending = host.status()?;
    for _ in 0..1000 {
        if let Some(response) = pending.try_recv()? {
            return match response?.value() {
                Response::Status(rui_session_shell::actor::OwnerStatus::Ready) => Ok(true),
                Response::Status(rui_session_shell::actor::OwnerStatus::Cleaning) => Ok(false),
                _ => Err("wire owner status failed".into()),
            };
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    Err("wire owner status reply deadline".into())
}
fn surface(state: &State, id: u64) -> Result<&Surface, Error> {
    match state.records.get(&id) {
        Some(Record::Surface(surface)) => Ok(surface),
        _ => Err("wire target surface missing".into()),
    }
}
fn pointer(
    client: &NetworkClient,
    opened: &OpenedView,
    lease: &p::ControlGranted,
    target: u64,
    sequence: u64,
) -> Result<Vec<u8>, Error> {
    let state = client.receiver().state()?;
    let surface = surface(state, target)?;
    let device = opened
        .input_devices
        .first()
        .ok_or("wire raw device grant missing")?;
    if device.text_owner != "HOST_PHYSICAL"
        || device.input_mode != "raw.host.v1"
        || opened.input_devices.len() != 1
    {
        return Err("wire device is not unique approved physical scope".into());
    }
    Ok(pb::RawInputBatch {
        header: Some(pb::CommandHeader {
            epoch: client.epoch.to_vec(),
            view_id: client.view_id.to_vec(),
            client_instance_id: CLIENT.to_vec(),
            command_seq: sequence,
            control_lease_id: lease.lease_id.clone(),
            control_generation: lease.generation,
            observed_view_revision: state.revision,
        }),
        surface_id: target,
        routing_token: surface.routing_token.clone(),
        presented_revision: state.revision,
        seat_id: device.seat_id.clone(),
        device_id: device.device_id.clone(),
        key_namespace: device.key_namespace.clone(),
        events: [true, false]
            .into_iter()
            .enumerate()
            .map(|(index, pressed)| pb::RawEvent {
                timestamp_us: sequence * 1000 + index as u64,
                event: Some(pb::raw_event::Event::Button(pb::ButtonEvent {
                    code: 1,
                    pressed,
                    position: Some(pb::Point { x: 20., y: 30. }),
                })),
            })
            .collect(),
    }
    .encode_to_vec())
}
async fn result(client: &mut NetworkClient, sequence: u64) -> Result<p::CommandResult, Error> {
    for _ in 0..256 {
        match event(client).await? {
            ClientEvent::CommandResult(result) if result.command_seq == sequence => {
                return Ok(result)
            }
            other => traffic(other)?,
        }
    }
    Err("wire command result packet ceiling".into())
}
async fn rejected_retry(
    client: &mut NetworkClient,
    bytes: &[u8],
    sequence: u64,
    reason: &str,
) -> Result<(), Error> {
    client.submit_family(CommandFamily::RawInput, bytes).await?;
    let first = result(client, sequence).await?;
    if first.status != "Rejected"
        || first.rejection != reason
        || !first.source_commit.is_empty()
        || first.resulting_view_revision.is_some()
    {
        return Err(format!(
            "wire negative status/authority mismatch: {} {}",
            first.status, first.rejection
        )
        .into());
    }
    client.submit_family(CommandFamily::RawInput, bytes).await?;
    if result(client, sequence).await? != first {
        return Err("exact rejected wire retry changed result".into());
    }
    Ok(())
}

pub struct Context {
    pub address: SocketAddr,
    pub credential: Credential,
    pub paint: PaintPort,
}
fn main_surface(state: &State) -> Result<u64, Error> {
    state
        .records
        .values()
        .find_map(|r| match r {
            Record::Surface(s) if s.focused => Some(s.id),
            _ => None,
        })
        .ok_or("wire main focus missing".into())
}
fn changed_effects(
    a: &rui_session_shell::actor::OwnerDiagnostics,
    b: &rui_session_shell::actor::OwnerDiagnostics,
) -> bool {
    a.native_frames_applied != b.native_frames_applied
        || b.held_keys != 0
        || b.held_buttons != 0
        || b.held_touches != 0
}
async fn no_native_effect(
    host: &HostPort,
    before: &rui_session_shell::actor::OwnerDiagnostics,
) -> Result<(), Error> {
    // Poll actual source owner after both ledger results. No pending native START
    // or EIS receipt is expected from a rejected request, even asynchronously.
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        if changed_effects(before, &diagnostics(host).await?) {
            return Err("rejected wire command produced native effects".into());
        }
    }
    Ok(())
}
fn source_effect(state: &State, id: u64) -> Result<([u8; 32], f64), Error> {
    let target = surface(state, id)?;
    let Some(Record::Scene(group)) = target.scene_root.and_then(|r| state.records.get(&r.id))
    else {
        return Err("wire effect root missing".into());
    };
    let activity = group
        .children
        .iter()
        .find_map(|r| match state.records.get(&r.id) {
            Some(Record::Scene(scene)) => match scene.primitive {
                rui_protocol_core::model::Primitive::SolidRect { color, .. }
                    if color.green == 0.25 && color.alpha == 1. =>
                {
                    Some(color.red)
                }
                _ => None,
            },
            _ => None,
        })
        .ok_or("wire activity indicator missing")?;
    Ok((Sha256::digest(text(state, id)?.as_bytes()).into(), activity))
}
async fn dispatched(
    client: &mut NetworkClient,
    host: &HostPort,
    bytes: &[u8],
    sequence: u64,
) -> Result<(p::CommandResult, rui_session_shell::actor::OwnerDiagnostics), Error> {
    let before = diagnostics(host).await?;
    client.submit_family(CommandFamily::RawInput, bytes).await?;
    let successful = result(client, sequence).await?;
    if successful.status != "Dispatched"
        || !successful.source_commit.is_empty()
        || successful.resulting_view_revision.is_some()
    {
        return Err(format!(
            "fresh native result must be Dispatched without APPLIED evidence, received {} {}",
            successful.status, successful.rejection
        )
        .into());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let d = diagnostics(host).await?;
        if d.native_frames_applied == before.native_frames_applied + 2 && d.held_buttons == 0 {
            return Ok((successful, d));
        }
        if Instant::now() >= deadline {
            return Err("fresh real wire press/release receipt missing".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
pub async fn run(
    context: &Context,
    port: &NativeProducerPort,
    host: &HostPort,
    root: &Path,
    observations: &[watch::Receiver<DesktopObservation>],
    local: &watch::Receiver<LocalObservation>,
    report: Option<&Path>,
) -> Result<(), Error> {
    let original = pair(port, observations).await?;
    let main = main_surface(original[0].capture.state())?;
    let before = diagnostics(host).await?;
    eprintln!("native wire: authenticated application-only denial");
    let mut denied = connect(context.address, &context.credential)
        .await
        .map_err(|e| format!("application-only connect: {e}"))?;
    eprintln!("native wire: application-only connected");
    let denial_memory = denied.receiver().memory_used();
    denied
        .open_view_with(&format!("application:{main}"), "host-canvas")
        .await?;
    let denial = match event(&mut denied).await {
        Ok(ClientEvent::Error(code)) if code == "AUTHORITY_REJECTED" => "Authority error reply",
        Err(error)
            if error.downcast_ref::<TransportError>().is_some_and(|e| {
                matches!(e, TransportError::Closed | TransportError::WebSocket)
            }) =>
        {
            "connection closure"
        }
        _ => {
            return Err(
                "application-only scope was not denied before any projection/resource".into(),
            )
        }
    };
    if denied.receiver().state()?.revision != 0
        || denied.receiver().cache_bytes()? != 0
        || denied.receiver().has_pending_publication()
        || denied.receiver().memory_used() != denial_memory
    {
        return Err("denied application-only scope received state/resource".into());
    }
    drop(denied);
    no_native_effect(host, &before).await?;
    let after_denial = source(port, &observations[0]).await?;
    if source_effect(after_denial.state(), main)?
        != source_effect(original[0].capture.state(), main)?
    {
        return Err("denied application scope changed authoritative source effects".into());
    }
    let mut client = connect(context.address, &context.credential).await?;
    eprintln!("native wire: fresh explicit session publication");
    client.open_view_with("session", "host-canvas").await?;
    let opened = ready(&mut client).await?;
    if opened.coverage != "complete" {
        return Err("wire probe incomplete source view".into());
    }
    paint(&mut client, &context.paint, port, 1).await?;
    let arranged = Command::new("/usr/bin/python3")
        .arg(root.join("fixtures/session-shell/native_session.py"))
        .arg("--arrange-probe")
        .output()?;
    if !arranged.status.success() {
        return Err("private wire probe placement failed".into());
    }
    // Probe creation/focus changes happen before any grant. Source frontend
    // callbacks remain enabled and their cleanup must settle normally.
    matrix::settle_source_focus(host, local).await?;
    let mut sequence = 1;
    let mut serial = 1;
    let mut reasons = Vec::new();
    for (index, cause) in ["move", "occlusion", "modal"].into_iter().enumerate() {
        sync_state(&mut client, port).await?;
        serial += 1;
        paint(&mut client, &context.paint, port, serial).await?;
        let lease = acquire(&mut client, 100 + index as u8).await?;
        // A real positive command with this same approved device/target/button
        // precedes every mutation. Invalid input cannot explain a negative.
        let positive_before = port.projected_source(CLIENT, client.view_id).await?;
        let effect_before = source_effect(positive_before.state(), main)?;
        let valid = pointer(&client, &opened, &lease, main, sequence)?;
        dispatched(&mut client, host, &valid, sequence).await?;
        sequence += 1;
        sync_state(&mut client, port).await?;
        serial += 1;
        paint(&mut client, &context.paint, port, serial).await?;
        let effect = source_effect(client.receiver().state()?, main)?;
        if effect.0 != effect_before.0 || effect.1 != effect_before.1 + 1. / 16. {
            return Err(
                "positive wire pointer did not cause exactly one source activity effect".into(),
            );
        }
        let before_state = surface(client.receiver().state()?, main)?.clone();
        let painted_revision = client.receiver().state()?.revision;
        let stale = pointer(&client, &opened, &lease, main, sequence)?;
        // The exact R1 command is captured from this receiver's actual painted
        // state before the source mutation. It is not re-encoded at R2.
        let native_before = diagnostics(host).await?;
        let mut transient = None;
        match cause {
            "move" => {
                edit(
                    host,
                    SourceEdit::Configure {
                        surface: main,
                        desktop_rect: Rect {
                            x: 13.,
                            ..before_state.desktop_rect
                        },
                        content_size: before_state.content_size,
                        transform: before_state.transform,
                        stacking_order: before_state.stacking_order,
                    },
                )
                .await?;
            }
            "occlusion" | "modal" => {
                let mut overlay = spec(
                    SurfaceRole::Popup,
                    20.,
                    20.,
                    70.,
                    65.,
                    20,
                    Color {
                        red: 0.2,
                        green: 0.6,
                        blue: 0.3,
                        alpha: 1.,
                    },
                );
                overlay.parent = Some(main);
                if cause == "modal" {
                    overlay.modal_for = Some(main);
                }
                let id = edit(host, SourceEdit::Open(overlay))
                    .await?
                    .ok_or("wire overlay ID missing")?;
                transient = Some(id);
                if cause == "modal" {
                    edit(host, SourceEdit::Focus(id)).await?;
                }
            }
            _ => unreachable!(),
        }
        let changed = pair(port, observations).await?;
        let new = surface(changed[0].capture.state(), main)?;
        if new.routing_token == before_state.routing_token {
            return Err("source mutation did not invalidate the R1 route".into());
        }
        if diagnostics(host)
            .await?
            .lease
            .as_ref()
            .is_none_or(|current| current.generation != lease.generation)
        {
            return Err(
                "stale route proof lost its original live lease before wire dispatch".into(),
            );
        }
        rejected_retry(&mut client, &stale, sequence, "InvalidArguments").await?;
        no_native_effect(host, &native_before).await?;
        let after_negative = port.projected_source(CLIENT, client.view_id).await?;
        if source_effect(after_negative.state(), main)? != effect {
            return Err("rejected route mutated authoritative text/activity".into());
        }
        // Publications may have advanced while the immutable R1 bytes awaited
        // dispatch, but no third-view Presented marker was manufactured at R2.
        if port.presented_revision(CLIENT, client.view_id).await? != Some(painted_revision) {
            return Err("R1 actual Presented marker unexpectedly changed".into());
        }
        reasons.push(cause);
        release(&mut client, &lease, 120 + index as u8, host).await?;
        if let Some(id) = transient {
            edit(host, SourceEdit::Close(id)).await?;
        }
        if cause == "move" {
            edit(
                host,
                SourceEdit::Configure {
                    surface: main,
                    desktop_rect: before_state.desktop_rect,
                    content_size: before_state.content_size,
                    transform: before_state.transform,
                    stacking_order: before_state.stacking_order,
                },
            )
            .await?;
        }
        edit(host, SourceEdit::Focus(main)).await?;
        pair(port, observations).await?;
        sequence += 1;
    }
    sync_state(&mut client, port).await?;
    serial += 1;
    paint(&mut client, &context.paint, port, serial).await?;
    let old = acquire(&mut client, 140).await?;
    let old_bytes = pointer(&client, &opened, &old, main, sequence)?;
    let original_generation = diagnostics(host).await?.native_work_generation;
    let callbacks = local.borrow().callbacks[2];
    let local_frame = *local.borrow();
    let scale = (f64::from(local_frame.width) / 200.).min(f64::from(local_frame.height) / 120.);
    let target = Point { x: 30., y: 40. };
    native(
        root,
        SOURCE,
        "click",
        &[
            (local_frame.origin.x
                + (f64::from(local_frame.width) - 200. * scale) / 2.
                + target.x * scale)
                .to_string(),
            (local_frame.origin.y
                + (f64::from(local_frame.height) - 120. * scale) / 2.
                + target.y * scale)
                .to_string(),
        ],
    )?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let local_after = loop {
        let d = diagnostics(host).await?;
        if local.borrow().callbacks[2] > callbacks
            && d.lease.is_none()
            && owner_ready(host).await?
            && d.native_work_generation > original_generation
        {
            if d.last_cleanup.as_ref().is_none_or(|c| {
                c.generation != old.generation
                    || c.reason != rui_protocol_core::session::CleanupReason::LocalActivity
            }) {
                return Err("actual local takeover did not cause old generation revocation".into());
            }
            break d;
        }
        if Instant::now() >= deadline {
            return Err("actual local takeover callback/cleanup deadline".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    rejected_retry(&mut client, &old_bytes, sequence, "StaleControl").await?;
    no_native_effect(host, &local_after).await?;
    sequence += 1;
    let final_pair = pair(port, observations).await?;
    // Move actual GDK focus away from source before a separate manual grant.
    native(root, CONTROLLER, "click", &point(&final_pair[0], target))?;
    matrix::settle_source_focus(host, local).await?;
    sync_state(&mut client, port).await?;
    serial += 1;
    paint(&mut client, &context.paint, port, serial).await?;
    let fresh = acquire(&mut client, 141).await?;
    if fresh.generation <= old.generation {
        return Err("manual fresh wire grant did not advance generation".into());
    }
    let valid = pointer(&client, &opened, &fresh, main, sequence)?;
    let before_fresh_effect = source_effect(client.receiver().state()?, main)?;
    let (successful, after) = dispatched(&mut client, host, &valid, sequence).await?;
    client
        .submit_family(CommandFamily::RawInput, &valid)
        .await?;
    if result(&mut client, sequence).await? != successful {
        return Err("fresh exact retry changed native dispatch result".into());
    }
    no_native_effect(host, &after).await?;
    sync_state(&mut client, port).await?;
    let after_fresh_effect = source_effect(client.receiver().state()?, main)?;
    if after_fresh_effect.0 != before_fresh_effect.0
        || after_fresh_effect.1 != before_fresh_effect.1 + 1. / 16.
    {
        return Err("manual fresh wire grant did not produce exactly one source effect".into());
    }
    release(&mut client, &fresh, 142, host).await?;
    let frames = pair(port, observations).await?;
    let receiver_budget = client.receiver().memory_budget();
    drop(opened);
    client.close().await?;
    context
        .paint
        .frames
        .try_send(UiRequest::Clear)
        .map_err(|_| "wire paint disposal slot unavailable")?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while context.paint.painted.borrow().is_some() || receiver_budget.used() != 0 {
        if Instant::now() >= deadline {
            return Err(format!(
                "wire receiver/native paint disposal retained {} bytes",
                receiver_budget.used()
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if let Some(report) = report {
        let mut out = File::create(report)?;
        writeln!(out,"{{\"scope\":\"actual independent third GTK receiver paint to TLS native source authority\",\"routing\":\"wire\",\"source_file_fingerprints\":{{{}}},\"application_only_denied_before_projection\":true,\"application_only_denial\":\"{denial}\",\"valid_native_preconditions\":3,\"stale_route_cases\":[\"{}\",\"{}\",\"{}\"],\"stale_results\":\"Rejected InvalidArguments\",\"rejected_exact_retries\":4,\"old_generation_after_actual_local_callback_denied\":true,\"manual_fresh_generation\":{},\"fresh_exact_retry_no_redispatch\":true,\"real_third_view_after_paints\":{serial},\"wire_receiver_disposal_bytes\":0,\"native_success_status\":\"Dispatched\",\"fresh_native_frames\":2,\"fresh_source_effect_delta\":1,\"negative_source_effects\":0,\"controller_records_sha256\":\"{}\",\"observer_records_sha256\":\"{}\",\"controller_after_paint_rgba_sha256\":\"{}\",\"observer_after_paint_rgba_sha256\":\"{}\"}}",source_hashes(root)?,reasons[0],reasons[1],reasons[2],fresh.generation,hex(&frames[0].records),hex(&frames[1].records),hex(&frames[0].pixels),hex(&frames[1].pixels))?;
    }
    Ok(())
}
