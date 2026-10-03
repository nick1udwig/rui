//! Full source/reference/two-window native integration on private Xvfb.
use rui_client_desktop::{
    desktop::{
        self, gtk, DesktopObservation, DisplayedObservation, Options, TypingNotice, ViewSelection,
    },
    *,
};
use rui_host_broker::{
    ControlledHost, HostConfig, NativeSourceFontProvider, SourceCapture, CANVAS_ID, CHECKBOX_ID,
    ITEM_IDS, LIST_ID, MODAL_TOGGLE_ID, POPUP_TOGGLE_ID, PUBLIC_TEXT_ID, PUBLIC_TEXT_MODEL_ID,
    REORDER_ID, TOOLTIP_TOGGLE_ID,
};
use rui_protocol_core::{
    commands::AuthenticatedClient,
    model::*,
    resources::{MemoryBudget, ResourceContext, ResourceScope},
    session::{Permissions, VerifiedAuthorization},
    transcript::{RecordingPolicy, TranscriptLimits},
};
use rui_transport::{
    diagnostics::Diagnostics,
    pairing::{Credential, PairingAuthority},
    HostIdentity, ServerConfig, ServerHandle,
};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;
#[path = "corpus_support/relay.rs"]
mod relay;
#[path = "corpus_support/trace.rs"]
mod trace;
type Error = Box<dyn std::error::Error + Send + Sync>;
const CONTROLLER: &str = "RUI corpus controller";
const OBSERVER: &str = "RUI corpus observer";
fn viewport_tuple(v: Viewport) -> (u32, u32, f64, f64, f64) {
    (v.width, v.height, v.origin.x, v.origin.y, v.scale)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn source_hashes(root: &Path) -> Result<serde_json::Value, Error> {
    let mut result = serde_json::Map::new();
    for name in [
        "client-desktop/src/desktop.rs",
        "client-desktop/src/desktop/gtk.rs",
        "client-desktop/src/source_controls.rs",
        "client-desktop/src/interaction.rs",
        "client-desktop/src/command_queue.rs",
        "client-desktop/examples/corpus_smoke.rs",
        "client-desktop/examples/corpus_support/trace.rs",
        "client-desktop/examples/corpus_support/relay.rs",
        "fixtures/render/native_events.py",
        "fixtures/render/native_font.py",
        "host-broker/src/source_input.rs",
        "host-broker/src/corpus.rs",
        "protocol-core/src/commands.rs",
        "protocol-core/src/session.rs",
        "protocol-core/src/receiver.rs",
        "transport/src/client.rs",
        "transport/src/server.rs",
        "transport/src/source_delivery.rs",
    ] {
        let mut file = File::open(root.join(name))?;
        let mut buffer = [0u8; 4096];
        let mut bytes = 0usize;
        let mut hash = Sha256::new();
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            bytes = bytes
                .checked_add(n)
                .filter(|n| *n <= 1024 * 1024)
                .ok_or("source fingerprint file ceiling")?;
            hash.update(&buffer[..n]);
        }
        result.insert(
            name.into(),
            serde_json::json!({"sha256":hex(&hash.finalize()),"bytes":bytes}),
        );
    }
    Ok(serde_json::Value::Object(result))
}
fn context(
    id: u8,
    observation: &watch::Receiver<DesktopObservation>,
) -> Result<ResourceContext, Error> {
    let binding = observation
        .borrow()
        .binding
        .ok_or("native trace binding unavailable")?;
    let digest = Sha256::digest(format!("native-corpus-{id}"));
    Ok(ResourceContext {
        scope: ResourceScope {
            session_id: [0; 16],
            epoch: binding.epoch,
            principal_id: digest[..16].try_into().unwrap(),
        },
        view_id: binding.view,
    })
}
fn export_trace(
    trace: &mut trace::Trace,
    diagnostics: &Diagnostics,
    id: u8,
    observation: &watch::Receiver<DesktopObservation>,
) -> Result<(), Error> {
    let expected = context(id, observation)?;
    let scope = diagnostics
        .scopes()?
        .into_iter()
        .find(|c| {
            c.view_id == expected.view_id
                && c.scope.epoch == expected.scope.epoch
                && c.scope.principal_id == expected.scope.principal_id
        })
        .ok_or("native trace scope unavailable")?;
    trace.export(diagnostics, scope)
}
struct Latencies {
    samples: Vec<(&'static str, i64, i64, i64, i64)>,
    _memory: rui_protocol_core::resources::MemoryReservation,
}
impl Latencies {
    fn new(budget: &MemoryBudget) -> Result<Self, Error> {
        let memory = budget.reserve(32 * 1024)?;
        let mut samples = Vec::new();
        samples.try_reserve_exact(128)?;
        Ok(Self {
            samples,
            _memory: memory,
        })
    }
    fn record(
        &mut self,
        label: &'static str,
        start: i64,
        controller: &ComparedFrame,
        observer: &ComparedFrame,
    ) -> Result<(), Error> {
        if self.samples.len() == 128 {
            return Err("native latency sample ceiling".into());
        }
        let c = controller.displayed.after_paint_time_us - start;
        let o = observer.displayed.after_paint_time_us - start;
        if c < 0 || o < 0 {
            return Err("native sample did not reach a causal after-paint".into());
        }
        self.samples.push((
            label,
            c,
            o,
            controller.displayed.refresh_interval_us,
            observer.displayed.refresh_interval_us,
        ));
        Ok(())
    }
    fn proof(&self) -> serde_json::Value {
        let distribution = |observer: bool| {
            let mut sorted = [0i64; 128];
            for (i, s) in self.samples.iter().enumerate() {
                sorted[i] = if observer { s.2 } else { s.1 };
            }
            sorted[..self.samples.len()].sort_unstable();
            let percentile = |p: usize| {
                if self.samples.is_empty() {
                    0
                } else {
                    sorted[(self.samples.len() * p).div_ceil(100).saturating_sub(1)]
                }
            };
            let known = self
                .samples
                .iter()
                .filter(|s| if observer { s.4 > 0 } else { s.3 > 0 })
                .count();
            let over = self
                .samples
                .iter()
                .filter(|s| {
                    if observer {
                        s.4 > 0 && s.2 > s.4
                    } else {
                        s.3 > 0 && s.1 > s.3
                    }
                })
                .count();
            serde_json::json!({"count":self.samples.len(),"p50_us":percentile(50),"p95_us":percentile(95),"p99_us":percentile(99),
                "samples_with_reported_refresh_interval":known,"over_one_reported_frame_interval":over})
        };
        serde_json::json!({"clock":"GLib monotonic microseconds","start":"actual GTK input callback before enqueue; canvas starts at release/submission callback; includes constrained network, source capture, resource verification, CPU render and actual GTK after-paint",
            "finish":"actual GTK after-paint callback for causal view revision; no physical scanout claim","configured_render_opportunity_hz":60,
            "controller":distribution(false),"observer":distribution(true),
            "warmup":"initial full resource delivery, viewport changes and native typing/Python scenario precede measured broader controls",
            "successful_sample_count":self.samples.len(),"failed_source_actions":0,"source_action_timeouts":0,
            "excluded_expected_rejections":["overflow before submission","Escape before submission"],
            "samples":self.samples.iter().map(|(action,c,o,ci,oi)|serde_json::json!({"action":action,"controller_us":c,"observer_us":o,
                "controller_reported_refresh_interval_us":ci,"observer_reported_refresh_interval_us":oi})).collect::<Vec<_>>()})
    }
}
struct PrivateFiles(PathBuf);
impl Drop for PrivateFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct SourceResources<'a>(&'a SourceCapture);
struct ComparedFrame {
    source: SourceCapture,
    displayed: DisplayedObservation,
    records: [u8; 32],
    pixels: [u8; 32],
}
impl ComparedFrame {
    fn proof(&self) -> serde_json::Value {
        let digest = |hash: &[u8; 32]| {
            hash.iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let stamp = self.displayed.stamp;
        serde_json::json!({
            "source_commit": self.source.source_commit(),
            "view_revision": stamp.revision,
            "frame_serial": stamp.serial,
            "record_count": self.source.state().records.len(),
            "verified_resource_count": self.source.resources().count(),
            "viewport": {"width":stamp.viewport.width,"height":stamp.viewport.height,
                "origin":[stamp.viewport.origin.x,stamp.viewport.origin.y],"scale":stamp.viewport.scale},
            "direct_source_records_sha256":digest(&self.records),
            "mirror_records_sha256":digest(&stamp.records_digest.unwrap()),
            "direct_source_rgba_sha256":digest(&self.pixels),
            "after_paint_rgba_sha256":digest(&stamp.pixel_digest),
            "actual_frame_clock":{"frame_time_us":self.displayed.frame_time_us,
                "refresh_interval_us":self.displayed.refresh_interval_us,
                "predicted_presentation_us":self.displayed.predicted_presentation_us,
                "after_paint_us":self.displayed.after_paint_time_us},
        })
    }
}
impl ResourceProvider for SourceResources<'_> {
    fn get(&self, digest: &[u8; 32]) -> Option<ResourceView<'_>> {
        self.0
            .resources()
            .find(|(offer, _)| &offer.digest == digest)
            .map(|(offer, bytes)| ResourceView { offer, bytes })
    }
}
fn native(root: &Path, title: &str, operation: &str, args: &[String]) -> Result<(), Error> {
    let output = Command::new("/usr/bin/python3")
        .arg(root.join("fixtures/render/native_events.py"))
        .arg(title)
        .arg(operation)
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "private native action failed: {}",
            String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(4096)])
        )
        .into());
    }
    Ok(())
}
fn keys(root: &Path, title: &str, args: &[&str]) -> Result<(), Error> {
    native(
        root,
        title,
        "keys",
        &args.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
    )
}
async fn wait(
    observation: &watch::Receiver<DesktopObservation>,
    mut condition: impl FnMut(DesktopObservation) -> bool,
) -> Result<DesktopObservation, Error> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let current = *observation.borrow();
        if current.closed || Instant::now() > deadline {
            return Err(format!("native observation deadline or closed view: {current:?}").into());
        }
        if condition(current) {
            return Ok(current);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
async fn capture(
    server: &ServerHandle,
    id: u8,
    observation: &watch::Receiver<DesktopObservation>,
) -> Result<SourceCapture, Error> {
    let binding = observation
        .borrow()
        .binding
        .ok_or("native connection binding unavailable")?;
    Ok(server.host.lock().await.source_capture(
        &AuthenticatedClient {
            principal: format!("native-corpus-{id}"),
            client_instance_id: [id; 16],
        },
        binding.view,
    )?)
}
async fn compare(
    server: &ServerHandle,
    id: u8,
    observation: &watch::Receiver<DesktopObservation>,
) -> Result<ComparedFrame, Error> {
    let source = capture(server, id, observation).await?;
    let revision = source.state().revision;
    let displayed = wait(observation, |o| {
        o.displayed.is_some_and(|d| d.stamp.revision == revision)
    })
    .await?
    .displayed
    .unwrap();
    assert_eq!(
        displayed.window_scale, 1,
        "pinned private native test uses scale1"
    );
    let records = desktop::records_digest(source.state()).ok_or("source fingerprint ceiling")?;
    assert_eq!(
        displayed.stamp.records_digest,
        Some(records),
        "actual mirror records diverged from authoritative capture"
    );
    let renderer = Renderer::new(RenderLimits::default(), MemoryBudget::new(128 << 20))?;
    let rgba = renderer
        .render_view(
            source.state(),
            displayed.stamp.viewport,
            &SourceResources(&source),
        )?
        .into_rgba8_over(Color {
            red: 1.0,
            green: 1.0,
            blue: 1.0,
            alpha: 1.0,
        })?;
    let digest: [u8; 32] = Sha256::digest(rgba.as_ref()).into();
    assert_eq!(
        displayed.stamp.pixel_digest, digest,
        "actual after-paint texture differs from direct source reference"
    );
    Ok(ComparedFrame {
        source,
        displayed,
        records,
        pixels: digest,
    })
}
async fn click(
    root: &Path,
    server: &ServerHandle,
    id: u8,
    observation: &watch::Receiver<DesktopObservation>,
    target: u64,
) -> Result<i64, Error> {
    let compared = compare(server, id, observation).await?;
    let source = compared.source;
    let displayed = compared.displayed;
    let binding = source
        .state()
        .records
        .values()
        .find_map(|record| {
            if let Record::Binding(b) = record {
                b.semantics.iter().any(|r| r.id == target).then_some(b)
            } else {
                None
            }
        })
        .ok_or("target has no source binding")?;
    let region = binding
        .interaction_region
        .ok_or("source binding has no interaction region")?;
    let Record::Surface(surface) = source
        .state()
        .records
        .get(&binding.surface.id)
        .ok_or("source surface absent")?
    else {
        return Err("source binding is not a surface".into());
    };
    let x = region.x + region.width / 2.0;
    let y = region.y + region.height / 2.0;
    let m = surface.transform;
    let desktop = Point {
        x: surface.desktop_rect.x + m.xx * x + m.xy * y + m.tx,
        y: surface.desktop_rect.y + m.yx * x + m.yy * y + m.ty,
    };
    let viewport = displayed.stamp.viewport;
    let px = displayed.canvas_origin.x + (desktop.x - viewport.origin.x) * viewport.scale;
    let py = displayed.canvas_origin.y + (desktop.y - viewport.origin.y) * viewport.scale;
    let start = gtk4::glib::monotonic_time();
    native(
        root,
        if id == 61 { CONTROLLER } else { OBSERVER },
        "click",
        &[px.to_string(), py.to_string()],
    )?;
    Ok(start)
}
async fn resize_canvas(
    root: &Path,
    title: &str,
    observation: &watch::Receiver<DesktopObservation>,
    width: u32,
    height: u32,
) -> Result<(), Error> {
    let displayed = wait(observation, |o| o.displayed.is_some())
        .await?
        .displayed
        .unwrap();
    let delta = [
        (width as i64 - displayed.stamp.viewport.width as i64).to_string(),
        (height as i64 - displayed.stamp.viewport.height as i64).to_string(),
    ];
    native(root, title, "resize_by", &delta)?;
    wait(observation, |o| {
        o.displayed
            .is_some_and(|d| d.stamp.viewport.width == width && d.stamp.viewport.height == height)
    })
    .await?;
    Ok(())
}
async fn text_converged(
    server: &ServerHandle,
    observation: &watch::Receiver<DesktopObservation>,
    expected: &str,
) -> Result<(), Error> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let source = capture(server, 61, observation).await?;
        let Record::Text(text) = source
            .state()
            .records
            .get(&PUBLIC_TEXT_MODEL_ID)
            .ok_or("public source text absent")?
        else {
            return Err("public source object is not text".into());
        };
        if text.content.as_deref() == Some(expected) && observation.borrow().typing_pending == 0 {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err("accepted rapid native text did not converge in order".into());
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
async fn changed(
    server: &ServerHandle,
    observations: &[watch::Receiver<DesktopObservation>],
    before: u64,
    label: &'static str,
    start: i64,
    latencies: &mut Latencies,
) -> Result<[ComparedFrame; 2], Error> {
    wait(&observations[0], |o| {
        o.has_control && o.input_ready && o.displayed.is_some_and(|d| d.stamp.revision > before)
    })
    .await?;
    let controller = compare(server, 61, &observations[0]).await?;
    let observer = compare(server, 62, &observations[1]).await?;
    let action = observations[0]
        .borrow()
        .last_action
        .ok_or("actual GUI input callback observation absent")?;
    if action.started_us < start || action.observed_revision != before {
        return Err("native callback is not bound to this causal source action".into());
    }
    latencies.record(label, action.started_us, &controller, &observer)?;
    Ok([controller, observer])
}
fn semantic(state: &State, id: u64) -> Result<&SemanticNode, Error> {
    match state.records.get(&id) {
        Some(Record::Semantic(n)) => Ok(n),
        _ => Err("expected source semantic record absent".into()),
    }
}
fn local_click(root: &Path, title: &str, point: Point) -> Result<i64, Error> {
    let start = gtk4::glib::monotonic_time();
    native(
        root,
        title,
        "click",
        &[point.x.to_string(), point.y.to_string()],
    )?;
    Ok(start)
}
fn spin(root: &Path, point: Point, value: u32) -> Result<(), Error> {
    local_click(root, CONTROLLER, point)?;
    let mut names = vec!["+Control_L".to_owned(), "a".into(), "-Control_L".into()];
    names.extend(value.to_string().chars().map(|c| c.to_string()));
    names.push("Tab".into());
    native(root, CONTROLLER, "keys", &names)
}
fn surface_point(
    source: &SourceCapture,
    displayed: DisplayedObservation,
    target: u64,
    offset: Point,
) -> Result<Point, Error> {
    let binding = source
        .state()
        .records
        .values()
        .find_map(|r| match r {
            Record::Binding(b) if b.semantics.iter().any(|r| r.id == target) => Some(b),
            _ => None,
        })
        .ok_or("source canvas binding absent")?;
    let region = binding
        .interaction_region
        .ok_or("source canvas input region absent")?;
    let Some(Record::Surface(surface)) = source.state().records.get(&binding.surface.id) else {
        return Err("source canvas surface absent".into());
    };
    let (x, y) = (region.x + offset.x, region.y + offset.y);
    let m = surface.transform;
    let desktop = Point {
        x: surface.desktop_rect.x + m.xx * x + m.xy * y + m.tx,
        y: surface.desktop_rect.y + m.yx * x + m.yy * y + m.ty,
    };
    let viewport = displayed.stamp.viewport;
    Ok(Point {
        x: displayed.canvas_origin.x + (desktop.x - viewport.origin.x) * viewport.scale,
        y: displayed.canvas_origin.y + (desktop.y - viewport.origin.y) * viewport.scale,
    })
}
async fn broad_controls(
    server: &ServerHandle,
    root: &Path,
    observations: &[watch::Receiver<DesktopObservation>],
    latencies: &mut Latencies,
) -> Result<serde_json::Value, Error> {
    eprintln!("native corpus: checkbox, stable reorder, scroll and transient source controls");
    for value in [true, false] {
        let before = observations[0].borrow().displayed.unwrap().stamp.revision;
        let start = click(root, server, 61, &observations[0], CHECKBOX_ID).await?;
        let frames = changed(server, observations, before, "checkbox", start, latencies).await?;
        assert_eq!(
            semantic(frames[0].source.state(), CHECKBOX_ID)?.value,
            Some(TypedValue::Boolean(value))
        );
    }
    let before = observations[0].borrow().displayed.unwrap().stamp.revision;
    let start = click(root, server, 61, &observations[0], ITEM_IDS[2]).await?;
    let frames = changed(
        server,
        observations,
        before,
        "stable-item-select",
        start,
        latencies,
    )
    .await?;
    assert_eq!(
        semantic(frames[0].source.state(), LIST_ID)?
            .collection
            .as_ref()
            .unwrap()
            .selected_items
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![ITEM_IDS[2]]
    );
    let ids = semantic(frames[0].source.state(), LIST_ID)?
        .collection
        .as_ref()
        .unwrap()
        .items
        .clone();
    drop(frames);
    let before = observations[0].borrow().displayed.unwrap().stamp.revision;
    let start = click(root, server, 61, &observations[0], REORDER_ID).await?;
    let frames = changed(
        server,
        observations,
        before,
        "stable-item-reorder",
        start,
        latencies,
    )
    .await?;
    let mut reordered = ids.clone();
    reordered.rotate_left(1);
    let collection = semantic(frames[0].source.state(), LIST_ID)?
        .collection
        .as_ref()
        .unwrap();
    assert_eq!(collection.items, reordered);
    assert_eq!(
        collection
            .selected_items
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![ITEM_IDS[2]]
    );
    drop(frames);
    click(root, server, 61, &observations[0], LIST_ID).await?;
    let controls = observations[0]
        .borrow()
        .local_controls
        .ok_or("native source controls unavailable")?;
    spin(root, controls.scroll_y, 60)?;
    let before = observations[0].borrow().displayed.unwrap().stamp.revision;
    let start = local_click(root, CONTROLLER, controls.scroll)?;
    let frames = changed(
        server,
        observations,
        before,
        "absolute-source-scroll",
        start,
        latencies,
    )
    .await?;
    let clipped = frames[0]
        .source
        .state()
        .records
        .values()
        .filter(|r| matches!(r,Record::Scene(n) if n.clip.is_some()))
        .count();
    assert!(clipped >= 3, "source list rows must remain clipped");
    drop(frames);
    // Restore materialized list visibility before later independent selection.
    click(root, server, 61, &observations[0], LIST_ID).await?;
    spin(root, controls.scroll_y, 0)?;
    let before = observations[0].borrow().displayed.unwrap().stamp.revision;
    let start = local_click(root, CONTROLLER, controls.scroll)?;
    drop(
        changed(
            server,
            observations,
            before,
            "source-scroll-restore",
            start,
            latencies,
        )
        .await?,
    );
    let mut retired = Vec::with_capacity(3);
    for (toggle, kind) in [
        (POPUP_TOGGLE_ID, "popup"),
        (MODAL_TOGGLE_ID, "modal"),
        (TOOLTIP_TOGGLE_ID, "tooltip"),
    ] {
        let before = observations[0].borrow().displayed.unwrap().stamp.revision;
        let start = click(root, server, 61, &observations[0], toggle).await?;
        let frames = changed(server, observations, before, kind, start, latencies).await?;
        let transient = frames[0]
            .source
            .state()
            .records
            .values()
            .find_map(|r| match r {
                Record::Surface(s) if s.parent.is_some() => Some(s),
                _ => None,
            })
            .ok_or("source transient absent")?;
        let id = transient.id;
        assert!(!retired.contains(&id));
        let dismiss = if kind == "tooltip" {
            toggle
        } else {
            transient
                .semantic_root
                .ok_or("transient semantic root absent")?
                .id
        };
        drop(frames);
        let before = observations[0].borrow().displayed.unwrap().stamp.revision;
        let start = click(root, server, 61, &observations[0], dismiss).await?;
        let frames = changed(
            server,
            observations,
            before,
            "transient-dismiss",
            start,
            latencies,
        )
        .await?;
        assert!(!frames[0].source.state().records.contains_key(&id));
        retired.push(id);
    }
    eprintln!("native corpus: bounded constrained-link canvas ordering and visible rejection");
    let source = compare(server, 61, &observations[0]).await?;
    let before = source.displayed.stamp.revision;
    let mut coordinates = Vec::with_capacity(24);
    for i in 0..12 {
        let p = surface_point(
            &source.source,
            source.displayed,
            CANVAS_ID,
            Point {
                x: 20.0 + i as f64 * 3.0,
                y: 60.0 + f64::from(i % 4) * 3.0,
            },
        )?;
        coordinates.extend([p.x.to_string(), p.y.to_string()]);
    }
    drop(source);
    let start = gtk4::glib::monotonic_time();
    native(root, CONTROLLER, "drag", &coordinates)?;
    let frames = changed(
        server,
        observations,
        before,
        "ordered-source-canvas",
        start,
        latencies,
    )
    .await?;
    let stroke = observations[0]
        .borrow()
        .last_stroke
        .ok_or("native canvas command was not observed")?;
    assert!(stroke.event_count >= 4 && stroke.event_count <= source_controls::MAX_STROKE_POINTS);
    let binding = observations[0].borrow().binding.unwrap();
    let host = server.host.lock().await;
    let trace = host.input_trace(
        &AuthenticatedClient {
            principal: "native-corpus-61".into(),
            client_instance_id: [61; 16],
        },
        binding.view,
    )?;
    let selected: Vec<_> = trace
        .iter()
        .filter(|s| s.command_sequence == stroke.command_sequence)
        .collect();
    assert_eq!(selected.len(), stroke.event_count);
    for (index, sample) in selected.iter().enumerate() {
        assert_eq!(sample.event_index as usize, index);
    }
    let actual = source_controls::pointer_event_digest(selected.iter().map(|s| s.event.clone()))
        .ok_or("source canvas trace is not pointer-only")?;
    assert_eq!(
        actual, stroke.events_digest,
        "every captured sample must survive constrained TLS in order"
    );
    let final_point = match selected.last().unwrap().event.event {
        rui_protocol_core::input::Event::Button {
            position,
            pressed: false,
            ..
        } => position,
        _ => return Err("source canvas missing terminal release".into()),
    };
    let Some(Record::Scene(shape)) = frames[0]
        .source
        .state()
        .records
        .get(&rui_host_broker::CANVAS_SHAPE_SCENE_ID)
    else {
        return Err("source shape absent".into());
    };
    let Primitive::SolidRect { rect, .. } = shape.primitive else {
        return Err("source shape is not a rectangle".into());
    };
    assert_eq!((rect.x, rect.y), (final_point.x, final_point.y));
    let source_before = host.diagnostics();
    drop(host);
    drop(frames);
    let source = compare(server, 61, &observations[0]).await?;
    let mut excessive = Vec::with_capacity(128);
    for i in 0..64 {
        let p = surface_point(
            &source.source,
            source.displayed,
            CANVAS_ID,
            Point {
                x: 20.0 + f64::from(i % 8) * 2.0,
                y: 60.0 + f64::from(i % 5) * 2.0,
            },
        )?;
        excessive.extend([p.x.to_string(), p.y.to_string()]);
    }
    drop(source);
    native(root, CONTROLLER, "drag", &excessive)?;
    wait(&observations[0], |o| {
        o.stroke_notice_visible && o.stroke_notice.is_some_and(|s| s.contains("sample limit"))
    })
    .await?;
    assert_eq!(
        server.host.lock().await.diagnostics().source_commit,
        source_before.source_commit
    );
    assert_eq!(
        server.host.lock().await.diagnostics().consumed_commands,
        source_before.consumed_commands
    );
    native(root, CONTROLLER, "drag_cancel", &coordinates)?;
    wait(&observations[0], |o| {
        o.stroke_notice_visible && o.stroke_notice == Some("Unsubmitted canvas stroke cancelled.")
    })
    .await?;
    assert_eq!(
        server.host.lock().await.diagnostics().source_commit,
        source_before.source_commit
    );
    eprintln!("native corpus: explicit source resize");
    let controls = observations[0].borrow().local_controls.unwrap();
    spin(root, controls.width, 700)?;
    spin(root, controls.height, 500)?;
    let before = observations[0].borrow().displayed.unwrap().stamp.revision;
    let observer_viewport = observations[1].borrow().displayed.unwrap().stamp.viewport;
    let start = local_click(root, CONTROLLER, controls.resize)?;
    let frames = changed(
        server,
        observations,
        before,
        "source-resize",
        start,
        latencies,
    )
    .await?;
    let Some(Record::Surface(surface)) = frames[0]
        .source
        .state()
        .records
        .get(&rui_host_broker::SURFACE_ID)
    else {
        return Err("resized source surface absent".into());
    };
    assert_eq!(
        (surface.content_size.width, surface.content_size.height),
        (700.0, 500.0)
    );
    assert_eq!(
        viewport_tuple(frames[1].displayed.stamp.viewport),
        viewport_tuple(observer_viewport)
    );
    Ok(
        serde_json::json!({"captured_event_count":stroke.event_count,"command_sequence":stroke.command_sequence,
        "captured_and_source_events_sha256":hex(&actual),"link_bytes_per_second_each_direction":relay::BYTES_PER_SECOND,
        "link_scope":"controller end-to-end TLS through opaque fixed-buffer byte-rate relay; healthy observer direct",
        "overflow":"64-point native drag exceeded capture+release ceiling; visible after-paint rejection; zero source commit/command",
        "cancel":"Escape before release; visible after-paint cancellation; zero source commit/command",
        "stable_item_ids":ITEM_IDS,"retired_transient_ids":retired,"source_resize":[700,500]}),
    )
}
async fn drive(
    server: &ServerHandle,
    root: &Path,
    directory: &Path,
    observations: &[watch::Receiver<DesktopObservation>],
    report: Option<&Path>,
    diagnostics: Option<&Diagnostics>,
) -> Result<(), Error> {
    let test_budget = MemoryBudget::new(1024 * 1024);
    let mut latencies = Latencies::new(&test_budget)?;
    let mut traces = if let Some(path) = report {
        Some([
            trace::Trace::new(
                &PathBuf::from(format!("{}.controller.trace.jsonl", path.display())),
                &test_budget,
            )?,
            trace::Trace::new(
                &PathBuf::from(format!("{}.observer.trace.jsonl", path.display())),
                &test_budget,
            )?,
        ])
    } else {
        None
    };
    eprintln!("native corpus: initial presentation and control");
    wait(&observations[0], |o| o.has_control && o.displayed.is_some()).await?;
    wait(&observations[1], |o| o.displayed.is_some()).await?;
    let initial = capture(server, 61, &observations[0]).await?;
    let canvas = initial
        .state()
        .records
        .values()
        .find_map(|r| {
            if let Record::Session(s) = r {
                Some(s.canvas)
            } else {
                None
            }
        })
        .ok_or("authoritative Session canvas absent")?;
    let width = canvas.width.ceil() as u32;
    let height = canvas.height.ceil() as u32;
    resize_canvas(root, CONTROLLER, &observations[0], width, height).await?;
    resize_canvas(root, OBSERVER, &observations[1], width, height).await?;
    let controller = compare(server, 61, &observations[0]).await?;
    let observer = compare(server, 62, &observations[1]).await?;
    drop((controller, observer));
    let before = server.host.lock().await.diagnostics();
    eprintln!("native corpus: source comparison and observer viewport");
    let observer_canvas = observations[1].borrow().displayed.unwrap().canvas_origin;
    native(
        root,
        OBSERVER,
        "click",
        &[
            (observer_canvas.x + 1.0).to_string(),
            (observer_canvas.y + 1.0).to_string(),
        ],
    )?;
    keys(
        root,
        OBSERVER,
        &["+Control_L", "Right", "Down", "equal", "-Control_L"],
    )?;
    wait(&observations[1], |o| {
        o.displayed.is_some_and(|d| {
            d.stamp.viewport.scale == 1.25
                && d.stamp.viewport.origin.x == 30.0
                && d.stamp.viewport.origin.y == 30.0
        })
    })
    .await?;
    resize_canvas(root, OBSERVER, &observations[1], width - 80, height - 40).await?;
    compare(server, 62, &observations[1]).await?;
    let after = server.host.lock().await.diagnostics();
    assert_eq!(before.source_commit, after.source_commit);
    assert_eq!(
        before.consumed_commands, after.consumed_commands,
        "observer viewport must not issue source commands"
    );
    click(root, server, 61, &observations[0], PUBLIC_TEXT_ID).await?;
    eprintln!("native corpus: rapid typing");
    wait(&observations[0], |o| {
        o.editing_target == Some(PUBLIC_TEXT_ID)
    })
    .await?;
    let source = capture(server, 61, &observations[0]).await?;
    let Record::Text(text) = source.state().records.get(&PUBLIC_TEXT_MODEL_ID).unwrap() else {
        panic!()
    };
    let original = text.content.as_deref().unwrap().to_owned();
    let selection = text.selection.unwrap();
    assert_eq!(
        selection,
        TextRange {
            anchor: 0,
            focus: 0
        }
    );
    keys(root, CONTROLLER, &["a", "b", "c", "d", "e", "f"])?;
    let mut expected = format!("abcdef{original}");
    text_converged(server, &observations[0], &expected).await?;
    compare(server, 61, &observations[0]).await?;
    compare(server, 62, &observations[1]).await?;
    // Repeat click/no-op Focus must rearm native typing rather than wait for an
    // editing-clock change that the source intentionally does not produce.
    click(root, server, 61, &observations[0], PUBLIC_TEXT_ID).await?;
    wait(&observations[0], |o| {
        o.editing_target == Some(PUBLIC_TEXT_ID)
    })
    .await?;
    keys(root, CONTROLLER, &["x"])?;
    expected = format!("abcdefx{original}");
    text_converged(server, &observations[0], &expected).await?;
    keys(
        root,
        CONTROLLER,
        &[
            "+Control_L",
            "+Shift_L",
            "u",
            "-Shift_L",
            "-Control_L",
            "0",
            "0",
            "e",
            "9",
            "Return",
        ],
    )?;
    expected = format!("abcdefxé{original}");
    text_converged(server, &observations[0], &expected).await?;
    compare(server, 61, &observations[0]).await?;
    compare(server, 62, &observations[1]).await?;
    keys(root, CONTROLLER, &["+Control_L", "Escape", "-Control_L"])?;
    eprintln!("native corpus: Python button, Unicode text and stable-item selection");
    wait(&observations[0], |o| o.control_revoked && !o.has_control).await?;
    let before_python = server.host.lock().await.diagnostics();
    let python_text = "Python AéΩ";
    let output = Command::new("/usr/bin/python3")
        .arg(root.join("conformance/semantic_client.py"))
        .args([
            "--port",
            &server.address.port().to_string(),
            "--certificate",
            directory.join("host.der").to_str().unwrap(),
            "--credential",
            directory.join("63.credential").to_str().unwrap(),
            "--instance",
            &"3f".repeat(16),
            "--activate-label",
            "Increment",
            "--replace-label",
            "Public text",
            "--replacement",
            python_text,
            "--select-item-label",
            "Item 2",
        ])
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "independent Python semantic scenario failed: {}",
            String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(4096)])
        )
        .into());
    }
    if output.stdout.len() > 4096 {
        return Err("independent Python report ceiling".into());
    }
    let python_report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(python_report["presented_acknowledgements"], 0);
    let results = python_report["command_results"]
        .as_array()
        .ok_or("independent Python command results absent")?;
    assert_eq!(results.len(), 3);
    for (index, result) in results.iter().enumerate() {
        assert_eq!(result["sequence"], index + 1);
        assert_eq!(result["status"], "Applied");
        assert!(result["source_commit"]
            .as_str()
            .is_some_and(|s| !s.is_empty()));
        assert!(result["revision"].as_u64().is_some_and(|r| r > 0));
    }
    let python_controller = compare(server, 61, &observations[0]).await?;
    let python_observer = compare(server, 62, &observations[1]).await?;
    for frame in [&python_controller, &python_observer] {
        let Some(Record::Text(text)) = frame.source.state().records.get(&PUBLIC_TEXT_MODEL_ID)
        else {
            return Err("public text source model absent".into());
        };
        assert_eq!(text.content.as_deref(), Some(python_text));
        let cursor = python_text.chars().count() as u64;
        assert_eq!(
            text.selection,
            Some(TextRange {
                anchor: cursor,
                focus: cursor
            })
        );
        let collection = semantic(frame.source.state(), LIST_ID)?
            .collection
            .as_ref()
            .ok_or("stable source collection absent")?;
        assert_eq!(collection.selected_items.len(), 1);
        assert_eq!(collection.selected_items[0].id, ITEM_IDS[1]);
    }
    let after_python = server.host.lock().await.diagnostics();
    assert_eq!(after_python.counter, before_python.counter + 1);
    assert_eq!(
        after_python.consumed_commands,
        before_python.consumed_commands + 3
    );
    assert_eq!(after_python.source_commit, before_python.source_commit + 3);
    let python_proof = serde_json::json!({
        "semantic_client": python_report,
        "source_effects": {"counter_increments":1,"consumed_commands":3,"source_commits":3,
            "button_exact_retry":"same APPLIED result; no second source effect",
            "public_text":"Unicode scalar replacement; exact source content and collapsed selection checked",
            "stable_item":"published Item 2 ID selected"},
        "controller":python_controller.proof(),
        "observer":python_observer.proof(),
    });
    drop((python_controller, python_observer));
    keys(root, CONTROLLER, &["+Control_L", "F2", "-Control_L"])?;
    eprintln!("native corpus: broker takeover and cancellation");
    wait(&observations[0], |o| o.has_control && o.input_ready).await?;
    let broad = broad_controls(server, root, observations, &mut latencies).await?;
    eprintln!("native corpus: actual cache loss and retained reconnect");
    let before = server.host.lock().await.diagnostics();
    let old = observations[1].borrow().displayed.unwrap().stamp;
    let controls = observations[1].borrow().local_controls.unwrap();
    local_click(root, OBSERVER, controls.cache)?;
    wait(&observations[1], |o| {
        o.input_ready
            && o.displayed
                .is_some_and(|d| d.stamp.generation > old.generation)
    })
    .await?;
    assert!(
        observations[0].borrow().has_control,
        "observer cache loss must not revoke controller"
    );
    compare(server, 62, &observations[1]).await?;
    let old = observations[0].borrow().displayed.unwrap().stamp;
    let controls = observations[0].borrow().local_controls.unwrap();
    local_click(root, CONTROLLER, controls.cache)?;
    wait(&observations[0], |o| {
        o.input_ready
            && o.displayed
                .is_some_and(|d| d.stamp.generation > old.generation)
    })
    .await?;
    compare(server, 61, &observations[0]).await?;
    if let (Some(traces), Some(diagnostics)) = (&mut traces, diagnostics) {
        export_trace(&mut traces[0], diagnostics, 61, &observations[0])?;
        traces[0].end_segment("retained prefix exported before explicit reconnect")?;
    }
    let old = observations[0].borrow().displayed.unwrap().stamp;
    local_click(root, CONTROLLER, controls.reconnect)?;
    wait(&observations[0], |o| {
        o.has_control
            && o.input_ready
            && o.displayed
                .is_some_and(|d| d.stamp.generation > old.generation)
    })
    .await?;
    let recovered_controller = compare(server, 61, &observations[0]).await?;
    assert_eq!(recovered_controller.displayed.stamp.revision, old.revision);
    assert_eq!(
        viewport_tuple(recovered_controller.displayed.stamp.viewport),
        viewport_tuple(old.viewport)
    );
    compare(server, 62, &observations[1]).await?;
    let after = server.host.lock().await.diagnostics();
    assert_eq!(
        before.source_commit, after.source_commit,
        "cache/reconnect must not mutate source"
    );
    assert_eq!(
        before.consumed_commands, after.consumed_commands,
        "retained reconnect must not redispatch source actions"
    );
    // A fresh actual frame and grant precede editing after reconnect; old local
    // composition and input generations were cleared by the worker.
    click(root, server, 61, &observations[0], PUBLIC_TEXT_ID).await?;
    wait(&observations[0], |o| {
        o.editing_target == Some(PUBLIC_TEXT_ID)
    })
    .await?;
    keys(root, CONTROLLER, &["g", "h", "i", "j", "k", "l"])?;
    wait(&observations[0], |o| o.typing_pending >= 2).await?;
    // The broker clamps this to its last observed transport clock; the test
    // does not advance the source clock or expire the lease artificially.
    server.host.lock().await.local_activity(0);
    wait(&observations[0], |o| {
        !o.has_control
            && o.notice_visible
            && matches!(
                o.typing_notice,
                TypingNotice::Cancelled | TypingNotice::Rejected
            )
    })
    .await?;
    assert_eq!(observations[0].borrow().typing_pending, 0);
    // No claim that cancellation can undo a command dispatched before takeover.
    let controller = compare(server, 61, &observations[0]).await?;
    let observer = compare(server, 62, &observations[1]).await?;
    if let Some(path) = report {
        let _memory = test_budget.reserve(512 * 1024)?;
        let mut trace_proofs = Vec::with_capacity(2);
        if let (Some(traces), Some(diagnostics)) = (&mut traces, diagnostics) {
            for (index, trace) in traces.iter_mut().enumerate() {
                export_trace(trace, diagnostics, 61 + index as u8, &observations[index])?;
                trace.end_segment("final exact retained prefix")?;
                trace_proofs.push(trace.proof()?);
            }
        }
        let mut cpuinfo = String::new();
        File::open("/proc/cpuinfo")?
            .take(4096)
            .read_to_string(&mut cpuinfo)?;
        let hardware = cpuinfo
            .lines()
            .find_map(|line| line.strip_prefix("model name\t: "))
            .unwrap_or("unknown")
            .to_owned();
        let timing = |o: DesktopObservation| {
            let t = o.render_timing;
            serde_json::json!({"accepted_timer_ticks":t.accepted_ticks,"late_timer_ticks":t.late_ticks,
            "elapsed_missed_timer_periods":t.elapsed_missed_periods,"attempted_frames":t.rendered_frames,"work_over_16_666_667ns":t.work_over_budget,"max_work_us":t.max_work_us,
            "scope":"worker timer with Skip behavior; CPU render+RGBA+diagnostic fingerprint work; actual after-paint is separate"})
        };
        let proof = serde_json::json!({
            "format":"rui.native-corpus-proof.v1",
            "result":"passed",
            "scope":"private Xvfb; shared GTK process; two independent TLS clients; GTK after-paint opportunity, not physical scanout",
            "checks":["full-session-source-reference","observer-viewport-isolation","rapid-six-character-typing",
                "repeat-focused-target","native-unicode-preedit-commit","independent-python-button-exact-retry",
                "independent-python-unicode-text","independent-python-stable-item",
                "checkbox-typed-value","stable-list-select-reorder","absolute-source-scroll","popup-modal-tooltip-retirement",
                "constrained-link-every-captured-canvas-sample","visible-overflow-and-escape-cancellation","explicit-source-resize",
                "actual-resource-cache-loss-two-views","retained-reconnect-fresh-after-paint-and-control",
                "broker-triggered-pending-cancellation-after-paint"],
            "hardware":{"cpu":hardware,"kernel":String::from_utf8(Command::new("uname").arg("-sr").output()?.stdout)?.trim(),"GTK":format!("{}.{}.{}",gtk4::major_version(),gtk4::minor_version(),gtk4::micro_version())},
            "canvas_and_controls":broad,"action_to_after_paint":latencies.proof(),
            "independent_python":python_proof,
            "build_profile":if cfg!(debug_assertions){"debug"}else{"release"},
            "source_file_fingerprints":source_hashes(root)?,
            "render_opportunity_timing":{"controller":timing(*observations[0].borrow()),"observer":timing(*observations[1].borrow())},
            "exact_post_auth_traces":{"controller":trace_proofs.first(),"observer":trace_proofs.get(1),"names":"<report>.controller.trace.jsonl and <report>.observer.trace.jsonl"},
            "controller":controller.proof(),
            "observer":observer.proof(),
        });
        serde_json::to_writer_pretty(File::create(path)?, &proof)?;
    }
    println!("native corpus: full {width}x{height} source/reference/two-mirror convergence; checkbox/list/scroll/transients/source resize; every canvas sample preserved across32KiB/s TLS with visible overflow/cancel; observer viewport isolated; native typing+Unicode+Python; actual cache loss+retained reconnect; visible takeover cancellation");
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("RUI_ISOLATED_DISPLAY").as_deref() != Ok("1") {
        return Err("use private native_smoke.py --probe corpus".into());
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let report = std::env::args().nth(1).map(PathBuf::from);
    let (options_tx, options_rx) = mpsc::sync_channel(1);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(async {
            let directory = std::env::temp_dir().join(format!(
                "rui-corpus-native-{}-{}",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
            ));
            std::fs::create_dir(&directory)?;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
            let _private = PrivateFiles(directory.clone());
            let cert = include_bytes!("../../transport/tests/data/localhost.der");
            File::create(directory.join("host.der"))?.write_all(cert)?;
            let mut pairing = PairingAuthority::new(4)?;
            let mut credentials = Vec::<Credential>::new();
            for id in [61u8, 62, 63] {
                let request = pairing.request_local_pairing([id; 16])?;
                let credential = pairing.approve_local(
                    request.request_id,
                    VerifiedAuthorization {
                        principal: format!("native-corpus-{id}"),
                        policy_version: 1,
                        permissions: Permissions {
                            can_view: true,
                            can_control: id != 62,
                            actions: if id == 62 {
                                Default::default()
                            } else if id == 63 {
                                ["Activate", "ReplaceText"]
                                    .into_iter()
                                    .map(str::to_owned)
                                    .collect()
                            } else {
                                [
                                    "Activate",
                                    "SetValue",
                                    "Focus",
                                    "ReplaceText",
                                    "SetSelection",
                                    "ScrollTo",
                                    "RawInputBatch",
                                    "TextComposition",
                                    "ResizeRequest",
                                ]
                                .into_iter()
                                .map(str::to_owned)
                                .collect()
                            },
                        },
                    },
                )?;
                if id == 63 {
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(directory.join("63.credential"))?
                        .write_all(credential.expose_for_tls_authentication())?;
                }
                credentials.push(credential);
            }
            let host = ControlledHost::new_corpus_with_fonts(
                HostConfig::fresh()?,
                "private-source".into(),
                Arc::new(NativeSourceFontProvider::new(
                    PathBuf::from("/usr/bin/python3"),
                    root.join("fixtures/render/native_font.py"),
                )),
            )?;
            let diagnostics = report
                .as_ref()
                .map(|_| {
                    Diagnostics::new(
                        RecordingPolicy::AuthorizedDiagnosticBytes,
                        TranscriptLimits {
                            records: 4096,
                            message_bytes: 1 << 20,
                            retained_bytes: 16 << 20,
                        },
                        4,
                    )
                })
                .transpose()?;
            let server = ServerHandle::bind(
                "127.0.0.1:0".parse()?,
                HostIdentity::from_der(
                    cert.to_vec(),
                    include_bytes!("../../transport/tests/data/localhost-key.der").to_vec(),
                )?,
                host,
                Arc::new(Mutex::new(pairing)),
                ServerConfig {
                    diagnostics: diagnostics.clone(),
                    ..ServerConfig::default()
                },
            )
            .await?;
            let relay = relay::Relay::new(server.address, &MemoryBudget::new(128 * 1024)).await?;
            let (stop, signal) = watch::channel(false);
            let mut options = Vec::new();
            let mut observations = Vec::new();
            for (index, credential) in credentials.into_iter().take(2).enumerate() {
                let (tx, rx) = watch::channel(DesktopObservation::default());
                observations.push(rx);
                options.push(Options {
                    address: if index == 0 {
                        relay.address
                    } else {
                        server.address
                    },
                    server_name: "localhost".into(),
                    certificate: cert.to_vec(),
                    credential,
                    client_id: [61 + index as u8; 16],
                    view_selection: ViewSelection::Fixture,
                    initial_scale: 1.0,
                    auto_control: index == 0,
                    smoke_frames: None,
                    title: if index == 0 { CONTROLLER } else { OBSERVER },
                    observations: Some(tx),
                    shutdown_signal: Some(signal.clone()),
                });
            }
            options_tx
                .send(options)
                .map_err(|_| "GTK option receiver closed")?;
            let result = drive(
                &server,
                &root,
                &directory,
                &observations,
                report.as_deref(),
                diagnostics.as_deref(),
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
            relay.shutdown().await;
            result
        });
        let _ = done_tx.send(result.map_err(|error: Error| error.to_string()));
    });
    let options = options_rx.recv_timeout(Duration::from_secs(30))?;
    gtk::run_many(options)?;
    thread.join().map_err(|_| "native source driver panicked")?;
    done_rx.recv()?.map_err(|message| message.into())
}
