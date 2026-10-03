//! Actual private GTK callbacks -> authenticated TLS -> same native EIS owner.
//! Three GTK windows share the test process; mirrors have independent TLS,
//! receiver, memory, permissions and viewport. No receipt is synthesized.
use gtk4::{gio, glib, graphene, prelude::*};
use rui_client_desktop::{
    desktop::{self, gtk, DesktopObservation, DisplayedObservation, Options, ViewSelection},
    RenderLimits, RenderProfile, Renderer, Viewport,
};
use rui_protocol_core::{
    model::{Affine, Color, Point, Record, Rect, Size, SurfaceRole, TypedValue},
    resources::MemoryBudget,
    session::{Permissions, VerifiedAuthorization},
};
use rui_session_shell::{
    actor::{
        transport::{NativeProducerPort, ProjectedCapture},
        HostPort, Owner, Response, SourceEdit,
    },
    source_window::{self, Painted},
    topology::WindowSpec,
    Framebuffer, InjectionScope, RegionGrant,
};
use rui_transport::{pairing::PairingAuthority, HostIdentity, ProducerServerHandle, ServerConfig};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::watch;
#[path = "native_session_smoke/matrix.rs"]
mod matrix;
#[path = "native_session_smoke/routing.rs"]
mod routing;
type Error = Box<dyn std::error::Error + Send + Sync>;
const CONTROLLER: &str = "RUI native session controller";
const OBSERVER: &str = "RUI native session observer";
const SOURCE: &str = "RUI isolated source probe";
// A failed test assertion must still stop the GTK loop. The runner separately
// terminates only this owned process group if toolkit shutdown stalls.
struct StopOnDrop(watch::Sender<bool>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn driver_panic_requests_gtk_shutdown() {
        let (stop, signal) = watch::channel(false);
        let failed = std::panic::catch_unwind(|| {
            let _shutdown = StopOnDrop(stop);
            panic!("bounded test driver failure");
        });
        assert!(failed.is_err());
        assert!(*signal.borrow());
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn source_hashes(root: &Path) -> Result<String, Error> {
    let mut output = String::new();
    for path in [
        "session-shell/examples/native_session_smoke.rs",
        "fixtures/session-shell/native_session.py",
        "session-shell/examples/native_session_smoke/matrix.rs",
        "fixtures/session-shell/native_held.py",
        "session-shell/examples/native_session_smoke/routing.rs",
        "fixtures/session-shell/native_mapping.py",
        "fixtures/session-shell/native_focus.py",
        "fixtures/render/native_events.py",
        "session-shell/src/actor.rs",
        "session-shell/src/actor/transport.rs",
        "session-shell/src/actor/transport/projection.rs",
        "session-shell/src/backend.rs",
        "session-shell/src/topology.rs",
        "session-shell/src/source_window.rs",
        "session-shell/native/bridge.c",
        "client-desktop/src/desktop.rs",
        "client-desktop/src/desktop/gtk.rs",
        "client-desktop/src/physical_dispatch.rs",
        "client-desktop/src/desktop/physical_callbacks.rs",
        "client-desktop/src/desktop/physical_continuity.rs",
        "client-desktop/src/desktop/physical_io.rs",
        "client-desktop/src/renderer.rs",
        "protocol-core/src/session.rs",
        "protocol-core/src/commands.rs",
        "protocol-core/src/input.rs",
        "transport/src/server.rs",
    ] {
        let mut file = File::open(root.join(path))?;
        let mut hash = Sha256::new();
        let mut total = 0usize;
        let mut buffer = [0u8; 4096];
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            total += n;
            if total > 1 << 20 {
                return Err("source fingerprint file ceiling".into());
            }
            hash.update(&buffer[..n]);
        }
        if !output.is_empty() {
            output.push(',');
        }
        output.push_str(&format!("{path:?}:\"{}\"", hex(&hash.finalize())));
    }
    Ok(output)
}
fn white() -> Color {
    Color {
        red: 1.,
        green: 1.,
        blue: 1.,
        alpha: 1.,
    }
}
fn same_native_point(actual: Point, expected: Point) -> bool {
    // EIS transmits absolute coordinates as float32. Bound the independent
    // inverse-map oracle below one ten-thousandth of a source logical unit.
    (actual.x - expected.x).abs() <= 0.0001 && (actual.y - expected.y).abs() <= 0.0001
}
fn identity() -> Affine {
    Affine {
        xx: 1.,
        yy: 1.,
        xy: 0.,
        yx: 0.,
        tx: 0.,
        ty: 0.,
    }
}
fn spec(role: SurfaceRole, x: f64, y: f64, w: f64, h: f64, z: i64, color: Color) -> WindowSpec {
    WindowSpec {
        role,
        parent: None,
        modal_for: None,
        desktop_rect: Rect {
            x,
            y,
            width: w,
            height: h,
        },
        content_size: Size {
            width: w,
            height: h,
        },
        transform: identity(),
        stacking_order: z,
        color,
    }
}
fn native(root: &Path, title: &str, operation: &str, args: &[String]) -> Result<(), Error> {
    let output = Command::new("/usr/bin/python3")
        .arg(root.join("fixtures/render/native_events.py"))
        .arg(title)
        .arg(operation)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "private native action failed ({operation}): {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}
fn held_key(root: &Path, pressed: bool) -> Result<(), Error> {
    let output = Command::new("/usr/bin/python3")
        .arg(root.join("fixtures/session-shell/native_focus.py"))
        .arg(CONTROLLER)
        .arg(if pressed { "down" } else { "up" })
        .output()?;
    if !output.status.success() {
        return Err("private physical y-key action failed".into());
    }
    Ok(())
}
async fn wait(
    observation: &watch::Receiver<DesktopObservation>,
    predicate: impl Fn(DesktopObservation) -> bool,
) -> Result<DesktopObservation, Error> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let current = *observation.borrow();
        if current.closed || Instant::now() >= deadline {
            return Err(format!("native mirror deadline/closed: {current:?}").into());
        }
        if predicate(current) {
            return Ok(current);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
async fn source(
    port: &NativeProducerPort,
    observation: &watch::Receiver<DesktopObservation>,
) -> Result<ProjectedCapture, Error> {
    let binding = observation
        .borrow()
        .binding
        .ok_or("missing actual mirror binding")?;
    Ok(port.projected_source(binding.client, binding.view).await?)
}
struct Compared {
    capture: ProjectedCapture,
    displayed: DisplayedObservation,
    records: [u8; 32],
    pixels: [u8; 32],
}
async fn compare(
    port: &NativeProducerPort,
    observation: &watch::Receiver<DesktopObservation>,
) -> Result<Compared, Error> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let capture = source(port, observation).await?;
        let displayed = wait(observation, |o| {
            o.displayed
                .is_some_and(|d| d.stamp.revision >= capture.state().revision)
        })
        .await?
        .displayed
        .unwrap();
        if displayed.stamp.revision != capture.state().revision {
            if Instant::now() >= deadline {
                return Err("source/mirror matching revision deadline".into());
            }
            continue;
        }
        let records =
            desktop::records_digest(capture.state()).ok_or("bounded source fingerprint")?;
        if displayed.stamp.records_digest != Some(records) {
            return Err("actual mirror records diverged from direct native projection".into());
        }
        let renderer = Renderer::new_with_profile(
            RenderLimits::default(),
            MemoryBudget::new(128 << 20),
            RenderProfile {
                cursor_vectors: true,
            },
        )?;
        let rgba = renderer
            .render_view(capture.state(), displayed.stamp.viewport, &())?
            .into_rgba8_over(white())?;
        let pixels = Sha256::digest(rgba.as_ref()).into();
        if pixels != displayed.stamp.pixel_digest {
            return Err(
                "actual after-paint texture diverged from direct native source reference".into(),
            );
        }
        return Ok(Compared {
            capture,
            displayed,
            records,
            pixels,
        });
    }
}
fn text(state: &rui_protocol_core::model::State, surface: u64) -> Result<&str, Error> {
    let Some(Record::Surface(surface)) = state.records.get(&surface) else {
        return Err("missing source surface".into());
    };
    let Some(Record::Semantic(node)) = surface
        .semantic_root
        .as_ref()
        .and_then(|r| state.records.get(&r.id))
    else {
        return Err("missing source semantic root".into());
    };
    match node.value.as_ref() {
        Some(TypedValue::Text(text)) => Ok(text),
        _ => Err("missing native public value".into()),
    }
}
fn point(frame: &Compared, position: Point) -> [String; 2] {
    let v = frame.displayed.stamp.viewport;
    [
        (frame.displayed.canvas_origin.x + (position.x - v.origin.x) * v.scale).to_string(),
        (frame.displayed.canvas_origin.y + (position.y - v.origin.y) * v.scale).to_string(),
    ]
}
async fn request_control(
    port: &NativeProducerPort,
    root: &Path,
    observation: &watch::Receiver<DesktopObservation>,
) -> Result<(), Error> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = *observation.borrow();
        let binding = state.binding.ok_or("actual view binding missing")?;
        let painted = state.displayed.ok_or("actual mirror paint missing")?;
        if port
            .presented_revision(binding.client, binding.view)
            .await?
            == Some(painted.stamp.revision)
        {
            break;
        }
        if Instant::now() >= deadline {
            return Err(
                "broker did not record the actual current Presented acknowledgement".into(),
            );
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let before = observation.borrow().control_requests;
    let p = observation
        .borrow()
        .local_controls
        .ok_or("actual control bounds missing")?
        .request_control;
    native(
        root,
        CONTROLLER,
        "click",
        &[p.x.to_string(), p.y.to_string()],
    )?;
    let deadline = Instant::now() + Duration::from_secs(4);
    while observation.borrow().control_requests == before {
        if Instant::now() >= deadline {
            return Err("actual GTK Request control callback was not delivered".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(())
}
async fn pair(
    port: &NativeProducerPort,
    observations: &[watch::Receiver<DesktopObservation>],
) -> Result<[Compared; 2], Error> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let a = compare(port, &observations[0]).await?;
        let b = compare(port, &observations[1]).await?;
        if a.capture.source_revision() != b.capture.source_revision() {
            if Instant::now() >= deadline {
                return Err("two-mirror source convergence deadline".into());
            }
            continue;
        }
        // The two bounded socket loops can coalesce different numbers of native
        // frames. Their record clocks describe each view's own publications. Each
        // exact view was checked above; compare common source content separately.
        let scratch = MemoryBudget::new(2 << 20);
        let _charged = scratch.reserve(2 << 20)?;
        let normalize = |capture: &ProjectedCapture| {
            let mut records = capture.records().to_vec();
            for record in &mut records {
                match record.record.as_mut().unwrap() {
                    rui_protocol_core::wire::proto::record::Record::Surface(r) => r.version = 1,
                    rui_protocol_core::wire::proto::record::Record::Scene(r) => r.version = 1,
                    rui_protocol_core::wire::proto::record::Record::Semantic(r) => r.version = 1,
                    rui_protocol_core::wire::proto::record::Record::Binding(r) => r.version = 1,
                    rui_protocol_core::wire::proto::record::Record::Session(r) => {
                        r.version = 1;
                        r.focus_generation = 1;
                    }
                    _ => unreachable!("bounded native source vocabulary"),
                }
            }
            records
        };
        if normalize(&a.capture) != normalize(&b.capture) {
            return Err("native controller/observer source content mismatch".into());
        }
        return Ok([a, b]);
    }
}
async fn diagnostics(port: &HostPort) -> Result<rui_session_shell::actor::OwnerDiagnostics, Error> {
    let pending = port.diagnostics()?;
    loop {
        match pending.try_recv()? {
            Some(response) => match response?.value() {
                Response::Diagnostics(d) => return Ok(d.clone()),
                _ => return Err("diagnostic reply kind".into()),
            },
            None => tokio::time::sleep(Duration::from_millis(1)).await,
        }
    }
}
async fn edit(port: &HostPort, edit: SourceEdit) -> Result<Option<u64>, Error> {
    let pending = port.source_edit(edit)?;
    loop {
        match pending.try_recv()? {
            Some(response) => match response?.value() {
                Response::Done => return Ok(None),
                Response::SurfaceOpened(id) => return Ok(Some(*id)),
                _ => return Err("source edit reply kind".into()),
            },
            None => tokio::time::sleep(Duration::from_millis(1)).await,
        }
    }
}
#[derive(Clone, Copy)]
struct LocalObservation {
    painted: Option<Painted>,
    callbacks: [u64; 3],
    origin: Point,
    width: i32,
    height: i32,
    failed: bool,
    active: bool,
    sampled_at_us: i64,
}
impl Default for LocalObservation {
    fn default() -> Self {
        Self {
            painted: None,
            callbacks: [0; 3],
            origin: Point { x: 0., y: 0. },
            width: 0,
            height: 0,
            failed: false,
            active: false,
            sampled_at_us: 0,
        }
    }
}
async fn drive(
    port: &NativeProducerPort,
    host: &HostPort,
    root: &Path,
    observations: &[watch::Receiver<DesktopObservation>],
    local: &watch::Receiver<LocalObservation>,
    report: Option<&Path>,
    scenario: (Option<&matrix::Context>, Option<&str>),
) -> Result<(), Error> {
    let (matrix, routing_mode) = scenario;
    eprintln!("native session: actual initial source/two-mirror presentation");
    let arranged = Command::new("/usr/bin/python3")
        .arg(root.join("fixtures/session-shell/native_session.py"))
        .arg("--arrange")
        .output()?;
    if !arranged.status.success() {
        return Err(format!(
            "private native window arrangement failed: {}",
            String::from_utf8_lossy(&arranged.stderr)
        )
        .into());
    }
    for o in observations {
        wait(o, |o| o.displayed.is_some()).await?;
    }
    let mut initial = pair(port, observations).await?;
    if routing_mode == Some("mapping") {
        routing::letterbox(root, &observations[0]).await?;
        routing::source_grid(root, local).await?;
        initial = pair(port, observations).await?;
    }
    let main = initial[0]
        .capture
        .state()
        .records
        .values()
        .find_map(|r| match r {
            Record::Surface(s) if s.focused => Some(s.id),
            _ => None,
        })
        .ok_or("source focus missing")?;
    assert_eq!(initial[0].displayed.stamp.viewport.scale, 1.5);
    assert!(initial[0]
        .capture
        .state()
        .records
        .values()
        .any(|r| matches!(r,Record::Surface(s) if s.role==SurfaceRole::Shell)));
    // These coordinates map to integer device pixels at the declared1.5
    // mirror scale, so the exact source oracle does not hide XTest rounding.
    let click = Point { x: 30., y: 64. };
    // Focus comes first. Source frontend focus loss is a real cleanup event,
    // so acquire explicitly after it settles instead of suppressing callbacks.
    native(root, CONTROLLER, "click", &point(&initial[0], click))?;
    matrix::settle_source_focus(host, local).await?;
    request_control(port, root, &observations[0]).await?;
    wait(&observations[0], |o| o.has_control && o.physical_ready).await?;
    let before = compare(port, &observations[0]).await?;
    assert_eq!(text(before.capture.state(), main)?, "");
    if routing_mode == Some("mapping") {
        return routing::mapping(port, host, root, observations, local, &before, report).await;
    }
    if let Some(context) = matrix {
        return matrix::run(matrix::Run {
            port,
            host,
            root,
            observations,
            local,
            before: &before,
            context,
            report,
        })
        .await;
    }
    // The native source only interprets physical keys. US physical y must be
    // German z; native client auto-repeat and local IM text cannot duplicate it.
    let baseline = diagnostics(host).await?;
    eprintln!("native session: actual GTK physical key hold and host repeat");
    held_key(root, true)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let captured = source(port, &observations[0]).await?;
        if text(captured.state(), main)?.len() >= 3 {
            break;
        }
        if Instant::now() >= deadline {
            let d = diagnostics(host).await?;
            return Err(format!("actual host physical repeat did not reach source; native_frames={} held_keys={} mirror={:?}",d.native_frames_applied,d.held_keys,*observations[0].borrow()).into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    held_key(root, false)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let after = loop {
        let d = diagnostics(host).await?;
        if d.held_keys == 0 {
            break d;
        }
        if Instant::now() >= deadline {
            return Err("physical release did not neutralize source".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert_eq!(
        after.native_frames_applied - baseline.native_frames_applied,
        2,
        "one native press and release, no client repeat sends"
    );
    let key_frames = pair(port, observations).await?;
    let value = text(key_frames[0].capture.state(), main)?;
    assert!(
        value.len() >= 3 && value.bytes().all(|b| b == b'z'),
        "host interpretation/repeat only"
    );
    let text_clock = key_frames[0].capture.source_revision();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        text(source(port, &observations[0]).await?.state(), main)?.len(),
        value.len(),
        "no repeat after physical release"
    );
    // Actual mirror scale1.5 inverse mapping -> independent EIS region. The
    // authoritative cursor must be transmitted, rendered and seen after-paint.
    eprintln!("native session: inverse pointer mapping and exported vector cursor");
    native(root, CONTROLLER, "click", &point(&key_frames[0], click))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let capture = source(port, &observations[0]).await?;
        if capture.source_revision() > text_clock
            && capture
                .state()
                .records
                .values()
                .any(|r| matches!(r,Record::Session(s) if s.cursor.as_ref().is_some_and(|c| same_native_point(c.position,click))))
        {
            break;
        }
        if Instant::now() >= deadline {
            let cursor = capture.state().records.values().find_map(|r| match r {
                Record::Session(s) => s.cursor.as_ref().map(|c| c.position),
                _ => None,
            });
            return Err(format!(
                "actual pointer source/cursor mapping mismatch; cursor={cursor:?} mirror={:?}",
                *observations[0].borrow()
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let pointer_frames = pair(port, observations).await?;
    assert_ne!(pointer_frames[0].pixels, key_frames[0].pixels);
    assert!(diagnostics(host).await?.held_buttons == 0);
    // Source-owned menu, tooltip, modal and scale/focus transitions. No mirror
    // business logic and no copying native effects into a controlled fixture.
    eprintln!("native session: source menu/tooltip/modal/focus/scale projection");
    let mut popup = spec(
        SurfaceRole::Popup,
        40.,
        25.,
        45.,
        35.,
        10,
        Color {
            red: 0.6,
            green: 0.2,
            blue: 0.2,
            alpha: 1.,
        },
    );
    popup.parent = Some(main);
    let popup = edit(host, SourceEdit::Open(popup)).await?.unwrap();
    let mut tooltip = spec(
        SurfaceRole::Tooltip,
        75.,
        7.,
        45.,
        12.,
        15,
        Color {
            red: 0.8,
            green: 0.8,
            blue: 0.,
            alpha: 1.,
        },
    );
    tooltip.parent = Some(main);
    let tooltip = edit(host, SourceEdit::Open(tooltip)).await?.unwrap();
    let popup_frames = pair(port, observations).await?;
    assert!(
        matches!(popup_frames[0].capture.state().records.get(&popup),Some(Record::Surface(s)) if s.role==SurfaceRole::Popup&&s.parent.is_some_and(|p|p.id==main))
    );
    assert!(
        matches!(popup_frames[0].capture.state().records.get(&tooltip),Some(Record::Surface(s)) if s.role==SurfaceRole::Tooltip&&s.input_region.is_empty())
    );
    let mut modal = spec(
        SurfaceRole::Popup,
        20.,
        18.,
        65.,
        60.,
        20,
        Color {
            red: 0.1,
            green: 0.7,
            blue: 0.2,
            alpha: 1.,
        },
    );
    modal.parent = Some(main);
    modal.modal_for = Some(main);
    let modal = edit(host, SourceEdit::Open(modal)).await?.unwrap();
    edit(host, SourceEdit::Focus(modal)).await?;
    let modal_frames = pair(port, observations).await?;
    assert!(
        matches!(modal_frames[0].capture.state().records.get(&modal),Some(Record::Surface(s)) if s.focused&&s.modal_for.is_some_and(|p|p.id==main))
    );
    let token = match modal_frames[0].capture.state().records.get(&main) {
        Some(Record::Surface(s)) => s.routing_token.clone(),
        _ => unreachable!(),
    };
    edit(host, SourceEdit::Close(modal)).await?;
    edit(host, SourceEdit::Close(popup)).await?;
    edit(host, SourceEdit::Close(tooltip)).await?;
    edit(
        host,
        SourceEdit::Configure {
            surface: main,
            desktop_rect: Rect {
                x: 8.,
                y: 12.,
                width: 130.,
                height: 85.,
            },
            content_size: Size {
                width: 100.,
                height: 90.,
            },
            transform: Affine {
                xx: 1.25,
                yy: 0.75,
                ..identity()
            },
            stacking_order: 2,
        },
    )
    .await?;
    let configured = pair(port, observations).await?;
    assert!(configured[0].capture.state().retired.contains(&modal));
    assert!(
        matches!(configured[0].capture.state().records.get(&main),Some(Record::Surface(s)) if s.focused&&s.transform.xx==1.25&&s.transform.yy==0.75&&s.routing_token!=token)
    );
    // Real source GDK callback is local takeover. Its press neutralizes shared
    // authority first, then source input is visible in both painted replicas.
    eprintln!("native session: actual source GDK local takeover");
    let deadline = Instant::now() + Duration::from_secs(5);
    let observation = loop {
        let o = *local.borrow();
        if o.failed {
            return Err("source frontend failed".into());
        }
        if o.painted.is_some() {
            break o;
        }
        if Instant::now() >= deadline {
            return Err("source after-paint unavailable".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let scale = (f64::from(observation.width) / 200.).min(f64::from(observation.height) / 120.);
    let local_point = [
        (observation.origin.x + (f64::from(observation.width) - 200. * scale) / 2. + 30. * scale)
            .to_string(),
        (observation.origin.y + (f64::from(observation.height) - 120. * scale) / 2. + 64. * scale)
            .to_string(),
    ];
    native(root, SOURCE, "click", &local_point)?;
    wait(&observations[0], |o| !o.has_control).await?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while local.borrow().callbacks[2] <= observation.callbacks[2] {
        if Instant::now() >= deadline {
            return Err("actual source callback missing".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let final_frames = pair(port, observations).await?;
    let d = diagnostics(host).await?;
    assert_eq!((d.held_keys, d.held_buttons, d.held_touches), (0, 0, 0));
    let renderer = Renderer::new_with_profile(
        RenderLimits::default(),
        MemoryBudget::new(128 << 20),
        RenderProfile {
            cursor_vectors: true,
        },
    )?;
    let full = renderer.render_view(
        final_frames[0].capture.state(),
        Viewport::new(200, 120),
        &(),
    )?;
    if let Some(report) = report {
        full.write_pam(&mut File::create(report.with_extension("pam"))?)?;
    }
    let final_rgba = full.into_rgba8_over(white())?;
    let digest: [u8; 32] = Sha256::digest(final_rgba.as_ref()).into();
    let deadline = Instant::now() + Duration::from_secs(5);
    let local_painted = loop {
        let o = *local.borrow();
        if o.failed {
            return Err("source frontend failed".into());
        }
        if let Some(p) = o.painted {
            if p.revision == final_frames[0].capture.source_revision() {
                break p;
            }
        }
        if Instant::now() >= deadline {
            return Err("matching source after-paint deadline".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert_eq!(local_painted.rgba8_sha256, digest);
    if let Some(report) = report {
        let mut file = File::create(report)?;
        writeln!(file,"{{\"scope\":\"private authenticated Xvfb; shared GTK process, two independent TLS mirrors and same native source owner; GTK after-paint opportunity, not scanout\",\"source_file_fingerprints\":{{{}}},\"mapping_error_tolerance\":0.0001,\"source_revision\":{},\"record_count\":{},\"host_layout\":\"de\",\"client_layout\":\"us\",\"keymap_sha256\":\"{}\",\"logical_region\":[0,0,200,120],\"EIS_region\":[40,80,600,240],\"framebuffer\":[1000,700],\"physical_scale\":2,\"native_repeat_delay_ms\":400,\"native_repeat_interval_ms\":40,\"controller_scale\":1.5,\"native_press_release_frames\":2,\"local_takeover_callbacks\":{},\"source_after_paint_rgba_sha256\":\"{}\",\"controller\":{{\"revision\":{},\"records_sha256\":\"{}\",\"after_paint_rgba_sha256\":\"{}\"}},\"observer\":{{\"revision\":{},\"records_sha256\":\"{}\",\"after_paint_rgba_sha256\":\"{}\"}}}}",
            source_hashes(root)?,final_frames[0].capture.source_revision(),final_frames[0].capture.state().records.len(),hex(&d.profile.keymap_sha256),local.borrow().callbacks[2],hex(&digest),final_frames[0].capture.state().revision,hex(&final_frames[0].records),hex(&final_frames[0].pixels),final_frames[1].capture.state().revision,hex(&final_frames[1].records),hex(&final_frames[1].pixels))?;
    }
    eprintln!("native session: same-source two-mirror records/pixels, physical callbacks/repeat, cursor, topology and local takeover passed");
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("RUI_ISOLATED_DISPLAY").as_deref() != Ok("1") {
        return Err("use private native_session.py runner".into());
    }
    gtk4::init()?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let report = std::env::args().nth(1).map(PathBuf::from);
    let matrix_mode = std::env::var("RUI_NATIVE_MATRIX")
        .ok()
        .map(|value| matrix::Mode::parse(&value))
        .transpose()
        .map_err(|e| e.to_string())?;
    let routing_mode = std::env::var("RUI_NATIVE_ROUTING").ok();
    if routing_mode
        .as_deref()
        .is_some_and(|mode| mode != "mapping")
    {
        return Err("unknown finite native routing case".into());
    }
    let budget = MemoryBudget::new(128 << 20);
    let disposal_budget = budget.clone();
    let owner = Owner::spawn(
        RegionGrant::new(
            InjectionScope::IsolatedWholeSession,
            Rect {
                x: 0.,
                y: 0.,
                width: 200.,
                height: 120.,
            },
            Rect {
                x: 40.,
                y: 80.,
                width: 600.,
                height: 240.,
            },
            Framebuffer {
                width: 1000,
                height: 700,
            },
            2.,
        )?,
        "de",
        vec![
            spec(
                SurfaceRole::Toplevel,
                10.,
                10.,
                100.,
                90.,
                0,
                Color {
                    red: 0.,
                    green: 0.,
                    blue: 1.,
                    alpha: 1.,
                },
            ),
            spec(
                SurfaceRole::Toplevel,
                110.,
                20.,
                80.,
                70.,
                1,
                Color {
                    red: 1.,
                    green: 0.,
                    blue: 0.,
                    alpha: 1.,
                },
            ),
            spec(
                SurfaceRole::Shell,
                0.,
                0.,
                200.,
                120.,
                -2,
                Color {
                    red: 0.05,
                    green: 0.05,
                    blue: 0.05,
                    alpha: 1.,
                },
            ),
        ],
        budget.clone(),
    )?;
    let native = NativeProducerPort::new(owner.port())?;
    let local_port = owner.local_device();
    let host = owner.port();
    let (local_tx, local_rx) = watch::channel(LocalObservation::default());
    let (stop, signal) = watch::channel(false);
    let source = Rc::new(RefCell::new(None::<source_window::SourceWindow>));
    let source_slot = source.clone();
    let source_signal = signal.clone();
    glib::idle_add_local_once(move || {
        let app = gio::Application::default()
            .unwrap()
            .downcast::<gtk4::Application>()
            .unwrap();
        let frontend =
            source_window::build(&app, local_port, budget, true).expect("private source frontend");
        frontend.window.present();
        source_slot.replace(Some(frontend));
        glib::timeout_add_local(Duration::from_millis(16), move || {
            if *source_signal.borrow() {
                source_slot.take();
                return glib::ControlFlow::Break;
            }
            if let Some(source) = source_slot.borrow().as_ref() {
                let origin = source
                    .canvas
                    .compute_point(&source.window, &graphene::Point::new(0., 0.))
                    .map(|p| Point {
                        x: f64::from(p.x()),
                        y: f64::from(p.y()),
                    })
                    .unwrap_or(Point { x: 0., y: 0. });
                local_tx.send_replace(LocalObservation {
                    painted: source.painted(),
                    callbacks: source.native_callbacks(),
                    origin,
                    width: source.canvas.width(),
                    height: source.canvas.height(),
                    failed: source.failure().is_some(),
                    active: source.window.is_active(),
                    sampled_at_us: glib::monotonic_time(),
                });
            }
            glib::ControlFlow::Continue
        });
    });
    let (options_tx, options_rx) = mpsc::sync_channel(1);
    let driver_report = report.clone();
    let thread = std::thread::spawn(move || {
        let _shutdown = StopOnDrop(stop.clone());
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let cert = include_bytes!("../../transport/tests/data/localhost.der");
            let mut pairing = PairingAuthority::new(2)?;
            let mut credentials = Vec::new();
            for id in [81u8, 82] {
                let request = pairing.request_local_pairing([id; 16])?;
                credentials.push(pairing.approve_local(
                    request.request_id,
                    VerifiedAuthorization {
                        principal: format!("native-session-{id}"),
                        policy_version: 1,
                        permissions: Permissions {
                            can_view: true,
                            can_control: id == 81,
                            actions: if id == 81 {
                                ["RawInputBatch".into()].into_iter().collect()
                            } else {
                                Default::default()
                            },
                        },
                    },
                )?);
            }
            let pairing = Arc::new(Mutex::new(pairing));
            let server = ProducerServerHandle::bind(
                "127.0.0.1:0".parse()?,
                HostIdentity::from_der(
                    cert.to_vec(),
                    include_bytes!("../../transport/tests/data/localhost-key.der").to_vec(),
                )?,
                Arc::new(native.clone()),
                pairing.clone(),
                ServerConfig::default(),
            )
            .await?;
            let matrix_context = match matrix_mode {
                Some(mode) => Some(matrix::Context {
                    mode,
                    relay: matrix::Relay::new(server.address).await?,
                    pairing,
                    credential: rui_transport::pairing::Credential::from_bytes(
                        *credentials[0].expose_for_tls_authentication(),
                    ),
                }),
                None => None,
            };
            let mut options = Vec::new();
            let mut observations = Vec::new();
            for (index, credential) in credentials.into_iter().enumerate() {
                let (tx, rx) = watch::channel(DesktopObservation::default());
                observations.push(rx);
                options.push(Options {
                    address: if index == 0 {
                        matrix_context
                            .as_ref()
                            .map_or(server.address, |context| context.relay.address)
                    } else {
                        server.address
                    },
                    server_name: "localhost".into(),
                    certificate: cert.to_vec(),
                    credential,
                    client_id: [81 + index as u8; 16],
                    view_selection: ViewSelection::WholeSession,
                    initial_scale: if index == 0 { 1.5 } else { 1. },
                    auto_control: false,
                    smoke_frames: None,
                    title: if index == 0 { CONTROLLER } else { OBSERVER },
                    observations: Some(tx),
                    shutdown_signal: Some(signal.clone()),
                });
            }
            options_tx
                .send(options)
                .map_err(|_| "GTK options channel closed")?;
            let result = drive(
                &native,
                &host,
                &root,
                &observations,
                &local_rx,
                driver_report.as_deref(),
                (matrix_context.as_ref(), routing_mode.as_deref()),
            )
            .await;
            stop.send_replace(true);
            for _ in 0..100 {
                if observations.iter().all(|o| o.borrow().closed) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            server.shutdown().await;
            result
        })
    });
    let options = options_rx.recv_timeout(Duration::from_secs(20))?;
    gtk::run_many(options)?;
    let result = thread.join().map_err(|_| "native session driver panic")?;
    source.borrow_mut().take();
    drop(source);
    drop(owner);
    let deadline = Instant::now() + Duration::from_secs(2);
    while disposal_budget.used() != 0 && Instant::now() < deadline {
        glib::MainContext::default().iteration(false);
        std::thread::sleep(Duration::from_millis(1));
    }
    if disposal_budget.used() != 0 {
        return Err(format!(
            "native shared owner disposal retained{}bytes",
            disposal_budget.used()
        )
        .into());
    }
    if result.is_ok() {
        if let Some(report) = &report {
            // Final disposal is recorded only after all escaped native GTK and
            // publication ownership has actually released its shared charge.
            let mut file = OpenOptions::new().read(true).write(true).open(report)?;
            let length = file.metadata()?.len();
            if !(2..=65504).contains(&length) {
                return Err("native report size ceiling".into());
            }
            file.seek(SeekFrom::End(-2))?;
            let mut tail = [0u8; 2];
            file.read_exact(&mut tail)?;
            if tail != *b"}\n" {
                return Err("native report termination mismatch".into());
            }
            file.seek(SeekFrom::End(-2))?;
            writeln!(file, ",\"owner_disposal_bytes\":0}}")?;
        }
    }
    result.map_err(|e| e.to_string().into())
}
