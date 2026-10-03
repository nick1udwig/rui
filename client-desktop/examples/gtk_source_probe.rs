//! Actual private GTK source -> pinned TLS -> GTK mirror after-paint oracle.
//! Source raster diagnostics remain local reference files, never resources.
use rui_client_desktop::{
    desktop::{self, gtk, DesktopObservation, DisplayedObservation, Options, ViewSelection},
    RenderLimits, Renderer, Viewport,
};
use rui_producer_sdk::{
    gtk_host::{GtkCommandHost, GtkProducerPort},
    CaptureArtifact,
};
use rui_protocol_core::{
    model::{Color, Point, Record},
    resources::{MemoryBudget, ResourceContext},
    session::{Permissions, VerifiedAuthorization},
    transcript::{RecordingPolicy, TranscriptLimits},
};
use rui_transport::{
    diagnostics::Diagnostics,
    pairing::{Credential, PairingAuthority},
    producer::ProducerPort,
    HostIdentity, ProducerServerHandle, ServerConfig,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::watch;
type Error = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, Error>;
const CONTROLLER: &str = "RUI native GTK source controller";
const OBSERVER: &str = "RUI native GTK source observer";
const CEILING: usize = 128 << 10;
#[path = "corpus_support/trace.rs"]
mod trace;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn approve(pairing: &mut PairingAuthority, id: u8, control: bool) -> Result<Credential> {
    let request = pairing.request_local_pairing([id; 16])?;
    Ok(pairing.approve_local(
        request.request_id,
        VerifiedAuthorization {
            principal: format!("native-gtk-source-{id}"),
            policy_version: 1,
            permissions: Permissions {
                can_view: true,
                can_control: control,
                actions: if control {
                    ["Activate", "Focus", "ReplaceText", "SetSelection"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect()
                } else {
                    BTreeSet::new()
                },
            },
        },
    )?)
}
fn native(root: &Path, title: &str, operation: &str, arguments: &[String]) -> Result<()> {
    let output = Command::new("python3")
        .arg(root.join("fixtures/render/native_events.py"))
        .arg(title)
        .arg(operation)
        .args(arguments)
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "owned display callback failed: {}",
            String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(4096)])
        )
        .into());
    }
    Ok(())
}
async fn wait(
    observation: &watch::Receiver<DesktopObservation>,
    condition: impl Fn(DesktopObservation) -> bool,
) -> Result<DesktopObservation> {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let current = *observation.borrow();
        if condition(current) {
            return Ok(current);
        }
        if current.closed || Instant::now() >= deadline {
            return Err(format!("actual mirror deadline/closed: {current:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
async fn enlarge(
    root: &Path,
    title: &str,
    observation: &watch::Receiver<DesktopObservation>,
) -> Result<()> {
    let displayed = wait(observation, |o| o.displayed.is_some())
        .await?
        .displayed
        .unwrap();
    // Normal desktop viewport, covering the entire720x600 source composition.
    native(
        root,
        title,
        "resize_by",
        &[
            (1024_i64 - displayed.stamp.viewport.width as i64).to_string(),
            (640_i64 - displayed.stamp.viewport.height as i64).to_string(),
        ],
    )?;
    wait(observation, |o| {
        o.displayed
            .is_some_and(|d| d.stamp.viewport.width == 1024 && d.stamp.viewport.height == 640)
    })
    .await?;
    Ok(())
}
fn latest(source: &Path) -> Result<PathBuf> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(source)?.take(16385) {
        if candidates.len() >= 64 {
            return Err("source capture history ceiling".into());
        }
        let path = entry?.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with("whole-")
            && name.ends_with(".json")
            && name != "whole-report.json"
            && name != "whole-failure.json"
        {
            candidates.push(path);
        }
    }
    candidates.sort();
    candidates
        .pop()
        .ok_or("current native whole-view capture missing".into())
}
fn retain(path: &Path, directory: &Path) -> Result<PathBuf> {
    let artifact = CaptureArtifact::read(path)?;
    let name = path.file_name().ok_or("native manifest filename")?;
    let target = directory.join(name);
    let label = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("native reference label")?;
    fs::copy(path, &target)?;
    fs::copy(path.with_extension("bin"), target.with_extension("bin"))?;
    let parent = path.parent().ok_or("native artifact parent")?;
    // Explicit source-owned local diagnostics; never passed to ProducerPort.
    for suffix in ["local.bgra", "local.png"] {
        let local = parent.join(format!("{label}.{suffix}"));
        if fs::metadata(&local)?.len() > 4 << 20 {
            return Err("local reference ceiling".into());
        }
        fs::copy(&local, directory.join(local.file_name().unwrap()))?;
    }
    let offers = artifact.manifest()["resource_offers"]
        .as_array()
        .ok_or("native offers")?;
    if offers.len() > 4096 {
        return Err("source closure ceiling".into());
    }
    let mut total = 0_u64;
    for offer in offers {
        let digest = offer["digest"]
            .as_str()
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("native digest")?;
        let name = format!("{digest}.rvp");
        let source = parent.join(&name);
        total = total
            .checked_add(fs::metadata(&source)?.len())
            .filter(|n| *n <= 16 << 20)
            .ok_or("native body quota")?;
        fs::copy(source, directory.join(name))?;
    }
    Ok(target)
}
struct Compared {
    proof: Value,
    close: Option<Point>,
}
async fn compare(
    path: &Path,
    directory: &Path,
    observation: &watch::Receiver<DesktopObservation>,
    budget: &MemoryBudget,
) -> Result<Compared> {
    // Bounded manifest allocation + private receiver + exact diagnostic scratch.
    let _working = budget.reserve(56 << 20)?;
    let artifact = CaptureArtifact::read(path)?;
    let mut receiver = artifact.receiver()?;
    let revision = artifact.apply_trusted_fixture(&mut receiver)?;
    let audit = artifact.verify_vector_audit(&receiver)?;
    let state = receiver.state()?;
    let displayed: DisplayedObservation = wait(observation, |o| {
        o.input_ready && o.displayed.is_some_and(|d| d.stamp.revision == revision)
    })
    .await?
    .displayed
    .unwrap();
    if displayed.window_scale != 1 || displayed.after_paint_time_us <= 0 {
        return Err("pinned actual GTK after-paint observation missing".into());
    }
    let records = desktop::records_digest(state).ok_or("record fingerprint ceiling")?;
    if displayed.stamp.records_digest != Some(records) {
        return Err("actual mirror full records differ from native source capture".into());
    }
    let renderer = Renderer::new(RenderLimits::default(), budget.clone())?;
    let rgba = renderer
        .render_view(state, displayed.stamp.viewport, &receiver)?
        .into_rgba8_over(Color {
            red: 1.0,
            green: 1.0,
            blue: 1.0,
            alpha: 1.0,
        })?;
    let digest: [u8; 32] = Sha256::digest(rgba.as_ref()).into();
    if digest != displayed.stamp.pixel_digest {
        return Err(
            "actual submitted GTK mirror texture differs from direct native-source projection"
                .into(),
        );
    }
    drop(rgba);
    // Export the same verified source frame with transparent background at its
    // native canvas size for the existing independent native-pixel comparator.
    let dimensions = artifact.manifest()["size"]
        .as_array()
        .ok_or("source size")?;
    let width = dimensions
        .first()
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or("source width")?;
    let height = dimensions
        .get(1)
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or("source height")?;
    let label = path.file_stem().ok_or("source frame label")?;
    let pam = directory.join(label).with_extension("pam");
    renderer
        .render_view(state, Viewport::new(width, height), &receiver)?
        .write_pam(&mut File::create(&pam)?)?;
    let close = state
        .records
        .values()
        .find_map(|record| match record {
            Record::Semantic(node) if node.label.as_deref() == Some("Close native modal") => {
                Some(node.id)
            }
            _ => None,
        })
        .and_then(|target| {
            state.records.values().find_map(|record| match record {
                Record::Binding(binding) if binding.semantics.iter().any(|r| r.id == target) => {
                    let Record::Surface(surface) = state.records.get(&binding.surface.id)? else {
                        return None;
                    };
                    let rect = binding.interaction_region?;
                    let point = Point {
                        x: rect.x + rect.width / 2.0,
                        y: rect.y + rect.height / 2.0,
                    };
                    let m = surface.transform;
                    let desktop = Point {
                        x: surface.desktop_rect.x + m.xx * point.x + m.xy * point.y + m.tx,
                        y: surface.desktop_rect.y + m.yx * point.x + m.yy * point.y + m.ty,
                    };
                    Some(Point {
                        x: displayed.canvas_origin.x
                            + (desktop.x - displayed.stamp.viewport.origin.x)
                                * displayed.stamp.viewport.scale,
                        y: displayed.canvas_origin.y
                            + (desktop.y - displayed.stamp.viewport.origin.y)
                                * displayed.stamp.viewport.scale,
                    })
                }
                _ => None,
            })
        });
    let stamp = displayed.stamp;
    Ok(Compared {
        close,
        proof: json!({
            "native_capture":path.file_name().unwrap().to_string_lossy(),"source_commit":artifact.manifest()["capture_token"],
            "view_revision":revision,"frame_serial":stamp.serial,"record_count":state.records.len(),
            "direct_source_records_sha256":hex(&records),"mirror_records_sha256":hex(&stamp.records_digest.unwrap()),
            "direct_source_rgba_sha256":hex(&digest),"actual_after_paint_rgba_sha256":hex(&stamp.pixel_digest),
            "viewport":{"width":stamp.viewport.width,"height":stamp.viewport.height,"origin":[stamp.viewport.origin.x,stamp.viewport.origin.y],"scale":stamp.viewport.scale},
            "frame_clock":{"frame_time_us":displayed.frame_time_us,"after_paint_time_us":displayed.after_paint_time_us,"refresh_interval_us":displayed.refresh_interval_us},
            "vector_audit":format!("{audit:?}"),"native_glyph_count":artifact.verify_native_glyph_evidence(&receiver)?,
            "local_pam":pam.file_name().unwrap().to_string_lossy(),"command_cause":artifact.manifest()["command_cause"],
        }),
    })
}
fn export(
    diagnostics: &Diagnostics,
    observation: &watch::Receiver<DesktopObservation>,
    directory: &Path,
    label: &str,
    budget: &MemoryBudget,
) -> Result<Value> {
    let binding = observation
        .borrow()
        .binding
        .ok_or("actual mirror scope absent")?;
    let context: ResourceContext = diagnostics
        .scopes()?
        .into_iter()
        .find(|scope| scope.view_id == binding.view && scope.scope.epoch == binding.epoch)
        .ok_or("retained diagnostic scope absent")?;
    let mut trace = trace::Trace::new(&directory.join(format!("{label}.trace.jsonl")), budget)?;
    trace.export(diagnostics, context)?;
    trace.end_segment("actual after-paint prefix before orderly mirror shutdown")?;
    trace.proof()
}
async fn drive(
    root: &Path,
    directory: &Path,
    source: &Path,
    observations: &[watch::Receiver<DesktopObservation>],
    diagnostics: &Diagnostics,
    budget: &MemoryBudget,
) -> Result<Value> {
    for observation in observations {
        wait(observation, |o| o.displayed.is_some()).await?;
    }
    // These titles identify only local windows for trusted isolated placement.
    // Protocol actions always resolve producer-issued semantic identities.
    for (title, x, y) in [(CONTROLLER, 0, 0), (OBSERVER, 1080, 0)] {
        let output = Command::new("python3")
            .arg(root.join("fixtures/gtk/place_windows.py"))
            .arg(title)
            .arg(x.to_string())
            .arg(y.to_string())
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "private fixture placement: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
    }
    for (index, observation) in observations.iter().enumerate() {
        enlarge(
            root,
            if index == 0 { CONTROLLER } else { OBSERVER },
            observation,
        )
        .await?;
    }
    wait(&observations[0], |o| o.has_control && o.input_ready).await?;
    let vector = directory.join("source-vector");
    fs::create_dir(&vector)?;
    let initial = retain(&latest(source)?, &vector)?;
    let controller = compare(&initial, &vector, &observations[0], budget).await?;
    let observer = compare(&initial, &vector, &observations[1], budget).await?;
    let close = controller
        .close
        .ok_or("published modal close region absent")?;
    // This native click enters the ordinary generic mirror hit-test/semantic
    // callback, shared authorization ledger and final native GTK source guard.
    let start = gtk4::glib::monotonic_time();
    native(
        root,
        CONTROLLER,
        "click",
        &[close.x.to_string(), close.y.to_string()],
    )?;
    wait(&observations[0], |o| {
        o.has_control && o.input_ready && o.displayed.is_some_and(|d| d.stamp.revision == 2)
    })
    .await?;
    let next = retain(&latest(source)?, &vector)?;
    let after_controller = compare(&next, &vector, &observations[0], budget).await?;
    let after_observer = compare(&next, &vector, &observations[1], budget).await?;
    let manifest = CaptureArtifact::read(&next)?;
    if manifest.manifest()["command_cause"]["authority_gate"] != "core-ledger-admitted" {
        return Err("native GTK effect lacks common-ledger causal capture".into());
    }
    Ok(
        json!({"mode":"actual-native-gtk-source-mirror","presentation_scope":"actual GTK Canvas snapshot then after-paint -> existing NetworkClient Presented -> same GtkCommandHost broker; no simulated callback","source_scope":"cooperating GTK4.22.4 fixture, immutable same-node local/export capture across registered native surfaces; isolated source-owned placement, no WM composition claim","observer_scope":"two separate instances of the same Rust receiver and renderer","initial":[controller.proof,observer.proof],"after_native_activate":[after_controller.proof,after_observer.proof],"source_action_started_us":start,"controller_trace":export(diagnostics,&observations[0],directory,"controller",budget)?,"observer_trace":export(diagnostics,&observations[1],directory,"observer",budget)?}),
    )
}
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    if std::env::var("RUI_ISOLATED_DISPLAY").as_deref() != Ok("1")
        || std::env::var_os("DISPLAY").is_none()
        || std::env::var_os("XAUTHORITY").is_none()
    {
        return Err("authenticated private harness display required".into());
    }
    let directory = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("fresh owned output directory required")?,
    );
    fs::create_dir_all(&directory)?;
    if fs::read_dir(&directory)?.next().is_some() {
        return Err("fresh output required".into());
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let (options_tx, options_rx) = mpsc::sync_channel(1);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(3)
            .enable_all()
            .build()
            .map_err(|e| -> Error { e.into() })
            .and_then(|runtime| {
                runtime.block_on(async {
                    let source = directory.join("source");
                    let mut command = Command::new("python3");
                    command
                        .arg(root.join("fixtures/gtk/whole_native_source.py"))
                        .arg("--output")
                        .arg(&source)
                        .arg("--fixture-hooks");
                    // Remote source and mirror never share a native input
                    // grab; both displays and authentication files are owned
                    // exclusively by this isolated harness.
                    command
                        .env("DISPLAY", std::env::var("RUI_GTK_SOURCE_DISPLAY")?)
                        .env("XAUTHORITY", std::env::var("RUI_GTK_SOURCE_XAUTHORITY")?);
                    let host = GtkCommandHost::spawn(command, &source)?;
                    let host_budget = host.memory_budget();
                    let port = Arc::new(GtkProducerPort::new(host)?);
                    let budget = MemoryBudget::new(128 << 20);
                    let mut pairing = PairingAuthority::new(4)?;
                    let credentials = [
                        approve(&mut pairing, 71, true)?,
                        approve(&mut pairing, 72, false)?,
                    ];
                    let diagnostics = Diagnostics::new(
                        RecordingPolicy::AuthorizedDiagnosticBytes,
                        TranscriptLimits {
                            retained_bytes: 1 << 20,
                            message_bytes: CEILING,
                            ..Default::default()
                        },
                        4,
                    )?;
                    let certificate =
                        include_bytes!("../../transport/tests/data/localhost.der").to_vec();
                    let server = ProducerServerHandle::bind(
                        "127.0.0.1:0".parse()?,
                        HostIdentity::from_der(
                            certificate.clone(),
                            include_bytes!("../../transport/tests/data/localhost-key.der").to_vec(),
                        )?,
                        port.clone(),
                        Arc::new(Mutex::new(pairing)),
                        ServerConfig {
                            max_connections: 4,
                            max_message_bytes: CEILING,
                            diagnostics: Some(diagnostics.clone()),
                            ..Default::default()
                        },
                    )
                    .await?;
                    let (stop, signal) = watch::channel(false);
                    let mut observations = Vec::new();
                    let mut options = Vec::new();
                    for (index, credential) in credentials.into_iter().enumerate() {
                        let (tx, rx) = watch::channel(DesktopObservation::default());
                        observations.push(rx);
                        options.push(Options {
                            address: server.address,
                            server_name: "localhost".into(),
                            certificate: certificate.clone(),
                            credential,
                            client_id: [71 + index as u8; 16],
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
                        &root,
                        &directory,
                        &source,
                        &observations,
                        &diagnostics,
                        &budget,
                    )
                    .await;
                    stop.send_replace(true);
                    for _ in 0..200 {
                        if observations.iter().all(|o| o.borrow().closed) {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    server.shutdown().await;
                    port.shutdown();
                    port.flush_lifecycle(0).await?;
                    drop(port);
                    drop(diagnostics);
                    let mut report = result?;
                    tokio::time::timeout(Duration::from_secs(3), async {
                        while host_budget.used() != 0 {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                    })
                    .await
                    .map_err(|_| {
                        format!(
                            "native owner budget survived disposal: {}",
                            host_budget.used()
                        )
                    })?;
                    report["host_memory_peak"] = json!(host_budget.peak());
                    report["host_memory_after_disposal"] = json!(host_budget.used());
                    report["diagnostic_memory_peak"] = json!(budget.peak());
                    fs::write(
                        directory.join("interaction.json"),
                        serde_json::to_vec_pretty(&report)?,
                    )?;
                    println!("actual native GTK source mirror PASS");
                    Ok::<_, Error>(())
                })
            });
        let _ = done_tx.send(result.map_err(|error| error.to_string()));
    });
    let options = match options_rx.recv_timeout(Duration::from_secs(12)) {
        Ok(options) => options,
        Err(error) => {
            if let Ok(Err(source_error)) = done_rx.recv_timeout(Duration::from_secs(1)) {
                return Err(source_error.into());
            }
            return Err(error.into());
        }
    };
    gtk::run_many(options)?;
    thread
        .join()
        .map_err(|_| "native GTK source driver panicked")?;
    done_rx.recv()?.map_err(|message| message.into())
}
