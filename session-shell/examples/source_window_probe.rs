//! Explicit private-display source probe; never uses the personal compositor.
use gtk4::{gio, glib, prelude::*};
use prost::Message;
use rui_client_desktop::{RenderLimits, RenderProfile, Renderer, Viewport};
use rui_protocol_core::{
    commands::CommandStatus,
    model::{Affine, Color, Record, Rect, Size, SurfaceRole, TypedValue},
    resources::MemoryBudget,
    session::{ClientHello, Permissions, ReceiverLimits, SessionConfig, VerifiedAuthorization},
    wire::proto as pb,
};
use rui_session_shell::{
    actor::{Connection, HostPort, Owner, Pending, Response},
    backend::{DEVICE, SEAT},
    source_window,
    topology::WindowSpec,
    Framebuffer, InjectionScope, RegionGrant,
};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    path::PathBuf,
    process::Command,
    rc::Rc,
    sync::{
        atomic::{AtomicI8, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn wait(
    pending: Pending,
) -> Result<rui_session_shell::actor::Received, Box<dyn std::error::Error>> {
    Ok(pending.wait(Duration::from_secs(2))?)
}
fn window(x: f64, y: f64, width: f64, height: f64, z: i64, color: Color) -> WindowSpec {
    WindowSpec {
        role: SurfaceRole::Toplevel,
        parent: None,
        modal_for: None,
        desktop_rect: Rect {
            x,
            y,
            width,
            height,
        },
        content_size: Size { width, height },
        transform: Affine {
            xx: 1.0,
            yy: 1.0,
            xy: 0.0,
            yx: 0.0,
            tx: 0.0,
            ty: 0.0,
        },
        stacking_order: z,
        color,
    }
}
fn prepare_remote(port: &HostPort) -> Result<Connection, Box<dyn std::error::Error>> {
    let config = SessionConfig::default();
    let mut profiles = config.supported_profiles;
    profiles.insert("cursor.vector.v1".into());
    let reply = wait(port.negotiate(
        &VerifiedAuthorization {
            principal: "private-source-probe".into(),
            policy_version: 1,
            permissions: Permissions {
                can_view: true,
                can_control: true,
                actions: ["RawInputBatch".into()].into_iter().collect(),
            },
        },
        &ClientHello {
            client_instance_id: [3; 16],
            supported_versions: [1].into_iter().collect(),
            supported_profiles: profiles,
            required_profiles: config.required_profiles,
            receiver_limits: ReceiverLimits::default(),
            resume: None,
        },
    )?)?;
    let Response::Connected { connection, .. } = reply.value() else {
        return Err("wrong negotiation reply".into());
    };
    let connection = connection.clone();
    let reply = wait(port.checkpoint(&connection)?)?;
    let Response::Publication(bytes) = reply.value() else {
        return Err("wrong checkpoint reply".into());
    };
    let envelope = pb::Envelope::decode(bytes.as_ref())?;
    let snapshot = pb::Snapshot::decode(envelope.payload.as_slice())?;
    // Admission fixture only: this is not a remote mirror presentation proof.
    wait(port.applied(&connection, snapshot.revision)?)?;
    wait(port.presented(&connection, snapshot.revision)?)?;
    let reply = wait(port.acquire_control(&connection, [9; 16])?)?;
    let Response::Lease(lease) = reply.value() else {
        return Err("wrong lease reply".into());
    };
    let lease = lease.clone();
    let source = wait(port.source_capture(&connection)?)?;
    let Response::Source(source) = source.value() else {
        return Err("wrong source reply".into());
    };
    let surface = source
        .state()
        .records
        .values()
        .find_map(|r| match r {
            Record::Surface(s) if s.focused => Some(s),
            _ => None,
        })
        .ok_or("no focused surface")?;
    let raw = pb::RawInputBatch {
        header: Some(pb::CommandHeader {
            epoch: connection.epoch().to_vec(),
            view_id: connection.view().to_vec(),
            client_instance_id: connection.client().client_instance_id.to_vec(),
            command_seq: 1,
            control_lease_id: lease.lease_id.to_vec(),
            control_generation: lease.generation,
            observed_view_revision: snapshot.revision,
        }),
        surface_id: surface.id,
        routing_token: surface.routing_token.clone(),
        presented_revision: snapshot.revision,
        seat_id: SEAT.into(),
        device_id: DEVICE.into(),
        key_namespace: "linux.evdev".into(),
        events: vec![pb::RawEvent {
            timestamp_us: 1000,
            event: Some(pb::raw_event::Event::Key(pb::KeyEvent {
                code: 42,
                pressed: true,
            })),
        }],
    }
    .encode_to_vec();
    let reply = wait(port.submit_raw(&connection, &raw)?)?;
    let Response::Command(command) = reply.value() else {
        return Err("wrong command reply".into());
    };
    assert_eq!(command.status, CommandStatus::Dispatched);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let reply = wait(port.diagnostics()?)?;
        let Response::Diagnostics(d) = reply.value() else {
            return Err("wrong diagnostic reply".into());
        };
        if d.held_keys == 1 {
            break;
        }
        if Instant::now() > deadline {
            return Err("remote Shift was not received".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(connection)
}
type Error = Box<dyn std::error::Error>;
struct Probe {
    application: gtk4::Application,
    source: Rc<RefCell<Option<source_window::SourceWindow>>>,
    port: HostPort,
    connection: Connection,
    budget: MemoryBudget,
    root: PathBuf,
    output: String,
    injected: Arc<AtomicI8>,
    phase: u8,
    pending: Option<Pending>,
    diagnostic: Option<Pending>,
    local_held: Option<usize>,
    quiet_since: Option<Instant>,
    focus_window: Option<gtk4::ApplicationWindow>,
    deadline: Instant,
}
impl Probe {
    fn action(&self, title: &'static str, operation: &'static str, completed: i8) {
        let script = self.root.join("fixtures/session-shell/native_focus.py");
        let injected = self.injected.clone();
        std::thread::spawn(move || {
            let result = Command::new("/usr/bin/python3")
                .arg(script)
                .args([title, operation])
                .status();
            injected.store(
                if result.is_ok_and(|status| status.success()) {
                    completed
                } else {
                    -1
                },
                Ordering::Release,
            );
        });
    }
    fn initial_keys(&self, source: &source_window::SourceWindow) -> Result<(), Error> {
        let bounds = source.painted().ok_or("no painted source")?.canvas;
        let scale = (source.canvas.width() as f64 / bounds.width)
            .min(source.canvas.height() as f64 / bounds.height);
        let point = gtk4::graphene::Point::new(
            ((source.canvas.width() as f64 - bounds.width * scale) / 2.0
                + (30.0 - bounds.x) * scale) as f32,
            ((source.canvas.height() as f64 - bounds.height * scale) / 2.0
                + (40.0 - bounds.y) * scale) as f32,
        );
        let point = source
            .canvas
            .compute_point(&source.window, &point)
            .ok_or("source canvas native placement unavailable")?;
        let x = point.x().to_string();
        let y = point.y().to_string();
        let script = self.root.join("fixtures/render/native_events.py");
        let injected = self.injected.clone();
        std::thread::spawn(move || {
            let click = Command::new("/usr/bin/python3")
                .arg(&script)
                .args(["RUI isolated source probe", "click", &x, &y])
                .status();
            let outcome = if click.as_ref().is_ok_and(|status| status.success()) {
                Command::new("/usr/bin/python3")
                    .arg(script)
                    .args(["RUI isolated source probe", "keys", "y", "y", "y"])
                    .status()
            } else {
                click
            };
            injected.store(
                if outcome.is_ok_and(|status| status.success()) {
                    1
                } else {
                    -1
                },
                Ordering::Release,
            );
        });
        Ok(())
    }
    fn step(&mut self) -> Result<bool, Error> {
        if Instant::now() > self.deadline {
            return Err("actual GDK takeover/source paint deadline".into());
        }
        if self.injected.load(Ordering::Acquire) < 0 {
            return Err("private native input action failed".into());
        }
        let source_owner = self.source.clone();
        let windows = source_owner.borrow();
        let source = windows.as_ref().ok_or("source window missing")?;
        if let Some(failure) = source.failure() {
            return Err(failure.into());
        }
        if self.phase == 0 && source.painted().is_some() {
            self.initial_keys(source)?;
            self.phase = 1;
        }
        if self.diagnostic.is_none() {
            self.diagnostic = Some(self.port.diagnostics()?);
        }
        if let Some(received) = self.diagnostic.as_ref().unwrap().try_recv()? {
            self.diagnostic.take();
            let received = received?;
            let Response::Diagnostics(diagnostics) = received.value() else {
                return Err("wrong diagnostic reply".into());
            };
            self.local_held = Some(diagnostics.local_held_keys);
        }
        if self.pending.is_none() {
            self.pending = Some(self.port.source_capture(&self.connection)?);
        }
        let Some(received) = self.pending.as_ref().unwrap().try_recv()? else {
            return Ok(false);
        };
        self.pending.take();
        let received = received?;
        let Response::Source(capture) = received.value() else {
            return Err("wrong capture reply".into());
        };
        let value = capture
            .state()
            .records
            .values()
            .find_map(|record| match record {
                Record::Semantic(node) => match &node.value {
                    Some(TypedValue::Text(text)) => Some(text.as_str()),
                    _ => None,
                },
                _ => None,
            })
            .ok_or("no source value")?;
        let painted = source
            .painted()
            .filter(|p| p.revision == capture.state().revision);
        match self.phase {
            1 if value == "zzz"
                && painted.is_some()
                && self.injected.load(Ordering::Acquire) == 1 =>
            {
                self.action("RUI isolated source probe", "down", 2);
                self.local_held = None;
                self.phase = 2;
            }
            2 if value == "zzzz"
                && painted.is_some()
                && self.local_held == Some(1)
                && self.injected.load(Ordering::Acquire) == 2 =>
            {
                let window = gtk4::ApplicationWindow::builder()
                    .application(&self.application)
                    .title("RUI source focus-away target")
                    .default_width(200)
                    .default_height(100)
                    .child(&gtk4::Button::with_label("Private focus target"))
                    .build();
                window.present();
                self.focus_window = Some(window);
                self.action("RUI source focus-away target", "focus", 3);
                self.local_held = None;
                self.phase = 3;
            }
            3 if self.local_held == Some(0) && self.injected.load(Ordering::Acquire) == 3 => {
                self.quiet_since = Some(Instant::now());
                self.phase = 4;
            }
            4 => {
                if value != "zzzz" {
                    return Err("source repeated after actual focus loss".into());
                }
                if self
                    .quiet_since
                    .is_some_and(|time| time.elapsed() >= Duration::from_millis(500))
                {
                    self.action("RUI source focus-away target", "up", 4);
                    self.phase = 5;
                }
            }
            5 if painted.is_some() && self.injected.load(Ordering::Acquire) == 4 => {
                if value != "zzzz" || self.local_held != Some(0) {
                    return Err("local key state survived focus loss".into());
                }
                self.verify(source, capture, painted.unwrap())?;
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }
    fn verify(
        &self,
        source: &source_window::SourceWindow,
        capture: &rui_session_shell::topology::Projection,
        painted: source_window::Painted,
    ) -> Result<(), Error> {
        let renderer = Renderer::new_with_profile(
            RenderLimits::default(),
            self.budget.clone(),
            RenderProfile {
                cursor_vectors: true,
            },
        )?;
        let frame = renderer.render_view(capture.state(), Viewport::new(200, 120), &())?;
        // Independent interior geometry assertions, outside cursor/AA bands.
        if frame.pixels()[50 * 200 + 15].rgba8() != [0, 0, 255, 255]
            || frame.pixels()[110 * 200 + 195].rgba8() != [0, 0, 0, 0]
        {
            return Err("independent source interior pixels differ".into());
        }
        frame.write_pam(&mut std::fs::File::create(&self.output)?)?;
        let rgba = frame.into_rgba8_over(Color {
            red: 1.0,
            green: 1.0,
            blue: 1.0,
            alpha: 1.0,
        })?;
        if painted.rgba8_sha256 != <[u8; 32]>::from(Sha256::digest(rgba.as_ref())) {
            return Err("painted pixels differ from direct source".into());
        }
        let snapshot = pb::Snapshot {
            revision: capture.state().revision,
            records: capture.records().to_vec(),
            ..Default::default()
        }
        .encode_to_vec();
        if painted.source_sha256 != <[u8; 32]>::from(Sha256::digest(snapshot)) {
            return Err("painted source revision/hash mismatch".into());
        }
        let callbacks = source.native_callbacks();
        if callbacks[0] != 4 || callbacks[1] != 3 || callbacks[2] < 1 {
            return Err("actual GDK focus-loss/key callback evidence differs".into());
        }
        let position = capture
            .state()
            .records
            .values()
            .find_map(|r| match r {
                Record::Session(s) => s.cursor.as_ref().map(|c| c.position),
                _ => None,
            })
            .ok_or("missing native cursor")?;
        if (position.x - 30.0).abs() > 0.5 || (position.y - 40.0).abs() > 0.5 {
            return Err("actual GDK pointer did not use source letterbox/grant mapping".into());
        }
        println!("{{\"scope\":\"private-GDK-owned-source-window\",\"admission_fixture\":\"trusted readiness; no remote mirror Presented claim\",\"local_input_origin\":\"actual authenticated private-X11 GDK physical callbacks\",\"focus_loss\":\"held physical key cleared; source stable for 500ms before native release\",\"host_layout\":\"de\",\"client_layout\":\"us\",\"physical_evdev_code\":21,\"native_key_presses\":{},\"native_key_releases\":{},\"native_pointer_callbacks\":{},\"source_revision\":{},\"source_records\":{},\"source_snapshot_sha256\":\"{}\",\"after_paint_rgba8_sha256\":\"{}\",\"accounted_bytes\":{},\"memory_limit_bytes\":{}}}", callbacks[0], callbacks[1], callbacks[2], painted.revision, capture.state().records.len(), hex(&painted.source_sha256), hex(&painted.rgba8_sha256), self.budget.used(), self.budget.limit());
        Ok(())
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        if self.phase >= 2 {
            let _ = Command::new("/usr/bin/python3")
                .arg(self.root.join("fixtures/session-shell/native_focus.py"))
                .args(["RUI source focus-away target", "up"])
                .status();
        }
        if let Some(window) = self.focus_window.take() {
            window.destroy();
        }
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("RUI_ISOLATED_DISPLAY").as_deref() != Ok("1") {
        return Err("launch through fixtures/session-shell/native_window.py".into());
    }
    let output = std::env::args()
        .nth(1)
        .ok_or("usage: source_window_probe OUTPUT.pam")?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let budget = MemoryBudget::new(128 << 20);
    let grant = RegionGrant::new(
        InjectionScope::IsolatedWholeSession,
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
        Framebuffer {
            width: 1000,
            height: 700,
        },
        2.0,
    )?;
    let mut popup = window(
        40.0,
        65.0,
        45.0,
        35.0,
        2,
        Color {
            red: 0.0,
            green: 0.5,
            blue: 0.0,
            alpha: 1.0,
        },
    );
    popup.role = SurfaceRole::Popup;
    popup.parent = Some(16);
    let mut tooltip = window(
        120.0,
        3.0,
        50.0,
        8.0,
        3,
        Color {
            red: 0.5,
            green: 0.5,
            blue: 0.5,
            alpha: 1.0,
        },
    );
    tooltip.role = SurfaceRole::Tooltip;
    tooltip.parent = Some(16);
    let owner = Owner::spawn(
        grant,
        "de",
        vec![
            window(
                10.0,
                10.0,
                120.0,
                90.0,
                0,
                Color {
                    red: 0.0,
                    green: 0.0,
                    blue: 1.0,
                    alpha: 1.0,
                },
            ),
            window(
                145.0,
                15.0,
                45.0,
                90.0,
                1,
                Color {
                    red: 0.5,
                    green: 0.0,
                    blue: 0.0,
                    alpha: 1.0,
                },
            ),
            popup,
            tooltip,
        ],
        budget.clone(),
    )?;
    let port = owner.port();
    let connection = prepare_remote(&port)?;
    let local = owner.local_device();
    let application = gtk4::Application::new(
        Some("dev.rui.IsolatedSourceProbe"),
        gio::ApplicationFlags::NON_UNIQUE,
    );
    let result = Rc::new(RefCell::new(None::<Result<(), String>>));
    let source_window = Rc::new(RefCell::new(None));
    let result_activate = result.clone();
    let window_activate = source_window.clone();
    let port_activate = port.clone();
    let connection_activate = connection.clone();
    let budget_activate = budget.clone();
    application.connect_activate(move |application| {
        let source =
            match source_window::build(application, local.clone(), budget_activate.clone(), true) {
                Ok(source) => source,
                Err(error) => {
                    result_activate.replace(Some(Err(error.to_string())));
                    application.quit();
                    return;
                }
            };
        source.window.present();
        source.canvas.grab_focus();
        window_activate.replace(Some(source));
        let mut probe = Probe {
            application: application.clone(),
            source: window_activate.clone(),
            port: port_activate.clone(),
            connection: connection_activate.clone(),
            budget: budget_activate.clone(),
            root: root.clone(),
            output: output.clone(),
            injected: Arc::new(AtomicI8::new(0)),
            phase: 0,
            pending: None,
            diagnostic: None,
            local_held: None,
            quiet_since: None,
            focus_window: None,
            deadline: Instant::now() + Duration::from_secs(15),
        };
        let result = result_activate.clone();
        let application = application.clone();
        glib::timeout_add_local(Duration::from_millis(16), move || match probe.step() {
            Ok(false) => glib::ControlFlow::Continue,
            Ok(true) => {
                result.replace(Some(Ok(())));
                application.quit();
                glib::ControlFlow::Break
            }
            Err(error) => {
                result.replace(Some(Err(error.to_string())));
                application.quit();
                glib::ControlFlow::Break
            }
        });
    });
    application.run_with_args::<&str>(&[]);
    let survivors = source_window
        .borrow()
        .as_ref()
        .map(|source| (source.window.clone(), source.canvas.clone()));
    source_window.borrow_mut().take();
    if let Some((window, canvas)) = survivors.as_ref() {
        assert!(window.child().is_none());
        assert_eq!(canvas.observe_controllers().n_items(), 0);
    }
    let retained_native = budget.used();
    drop(survivors);
    for _ in 0..32 {
        if !glib::MainContext::default().pending() {
            break;
        }
        glib::MainContext::default().iteration(false);
    }
    assert!(
        retained_native.saturating_sub(budget.used()) >= 4 << 20,
        "cloned native objects did not retain/release opaque allowance"
    );
    let diagnostic_reply = wait(port.diagnostics()?)?;
    let Response::Diagnostics(diagnostics) = diagnostic_reply.value() else {
        return Err("wrong final diagnostics".into());
    };
    assert_eq!(
        (
            diagnostics.held_keys,
            diagnostics.held_buttons,
            diagnostics.held_touches
        ),
        (0, 0, 0)
    );
    assert_eq!(diagnostics.profile.host_layout, "de");
    assert_eq!(diagnostics.local_held_keys, 0);
    println!("{{\"native_owner_cleanup\":\"actual releases and STOP verified\",\"held_keys\":{},\"held_buttons\":{},\"held_touches\":{},\"local_held_keys\":{},\"native_clone_allowance\":\"retained until final native object clones dropped\",\"libei\":\"{}\",\"libeis\":\"{}\",\"xkbcommon\":\"{}\",\"keymap_sha256\":\"{}\",\"native_frames_applied\":{},\"native_frames_cancelled\":{}}}",diagnostics.held_keys,diagnostics.held_buttons,diagnostics.held_touches,diagnostics.local_held_keys,diagnostics.profile.libei,diagnostics.profile.libeis,diagnostics.profile.xkbcommon,hex(&diagnostics.profile.keymap_sha256),diagnostics.native_frames_applied,diagnostics.native_frames_cancelled);
    owner.shutdown();
    drop(diagnostic_reply);
    drop(connection);
    drop(port);
    drop(application);
    let outcome = result
        .borrow_mut()
        .take()
        .unwrap_or_else(|| Err("source probe ended without proof".into()))
        .map_err(Into::into);
    outcome
}
