//! Full source/reference/two-window native integration on private Xvfb.
use rui_client_desktop::{
    desktop::{self, gtk, DesktopObservation, DisplayedObservation, Options, TypingNotice},
    *,
};
use rui_host_broker::{
    ControlledHost, HostConfig, NativeSourceFontProvider, SourceCapture, PUBLIC_TEXT_ID,
    PUBLIC_TEXT_MODEL_ID,
};
use rui_protocol_core::{
    commands::AuthenticatedClient,
    model::*,
    resources::MemoryBudget,
    session::{Permissions, VerifiedAuthorization},
};
use rui_transport::{
    pairing::{Credential, PairingAuthority},
    HostIdentity, ServerConfig, ServerHandle,
};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;
type Error = Box<dyn std::error::Error + Send + Sync>;
const CONTROLLER: &str = "RUI corpus controller";
const OBSERVER: &str = "RUI corpus observer";
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
) -> Result<(), Error> {
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
    native(
        root,
        if id == 61 { CONTROLLER } else { OBSERVER },
        "click",
        &[px.to_string(), py.to_string()],
    )?;
    Ok(())
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
async fn drive(
    server: &ServerHandle,
    root: &Path,
    directory: &Path,
    observations: &[watch::Receiver<DesktopObservation>],
    report: Option<&Path>,
) -> Result<(), Error> {
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
    eprintln!("native corpus: Python stable-item selection");
    wait(&observations[0], |o| o.control_revoked && !o.has_control).await?;
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
            "--select-item-label",
            "Item 2",
        ])
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "independent Python item selection failed: {}",
            String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(4096)])
        )
        .into());
    }
    compare(server, 61, &observations[0]).await?;
    compare(server, 62, &observations[1]).await?;
    keys(root, CONTROLLER, &["+Control_L", "F2", "-Control_L"])?;
    eprintln!("native corpus: broker takeover and cancellation");
    wait(&observations[0], |o| o.has_control).await?;
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
        let budget = MemoryBudget::new(32 * 1024);
        let _memory = budget.reserve(32 * 1024)?;
        let proof = serde_json::json!({
            "format":"rui.native-corpus-proof.v1",
            "result":"passed",
            "scope":"private Xvfb; shared GTK process; two independent TLS clients; GTK after-paint opportunity, not physical scanout",
            "checks":["full-session-source-reference","observer-viewport-isolation","rapid-six-character-typing",
                "repeat-focused-target","native-unicode-preedit-commit","independent-python-stable-item",
                "broker-triggered-pending-cancellation-after-paint"],
            "controller":controller.proof(),
            "observer":observer.proof(),
        });
        serde_json::to_writer_pretty(File::create(path)?, &proof)?;
    }
    println!("native corpus: full {width}x{height} source/reference/two-mirror convergence; observer pan/zoom/resize isolated; rapid six-character typing + repeat focus + Unicode preedit; independent Python stable-item selection; takeover cancelled pending typing visibly");
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
                                ["Activate".into()].into_iter().collect()
                            } else {
                                [
                                    "Activate",
                                    "SetValue",
                                    "Focus",
                                    "ReplaceText",
                                    "SetSelection",
                                    "ScrollTo",
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
                HostConfig::default(),
                "private-source".into(),
                Arc::new(NativeSourceFontProvider::new(
                    PathBuf::from("/usr/bin/python3"),
                    root.join("fixtures/render/native_font.py"),
                )),
            )?;
            let server = ServerHandle::bind(
                "127.0.0.1:0".parse()?,
                HostIdentity::from_der(
                    cert.to_vec(),
                    include_bytes!("../../transport/tests/data/localhost-key.der").to_vec(),
                )?,
                host,
                Arc::new(Mutex::new(pairing)),
                ServerConfig::default(),
            )
            .await?;
            let (stop, signal) = watch::channel(false);
            let mut options = Vec::new();
            let mut observations = Vec::new();
            for (index, credential) in credentials.into_iter().take(2).enumerate() {
                let (tx, rx) = watch::channel(DesktopObservation::default());
                observations.push(rx);
                options.push(Options {
                    address: server.address,
                    server_name: "localhost".into(),
                    certificate: cert.to_vec(),
                    credential,
                    client_id: [61 + index as u8; 16],
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
            let result = drive(&server, &root, &directory, &observations, report.as_deref()).await;
            stop.send_replace(true);
            for _ in 0..100 {
                if observations.iter().all(|o| o.borrow().closed) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            server.shutdown().await;
            result
        });
        let _ = done_tx.send(result.map_err(|error: Error| error.to_string()));
    });
    let options = options_rx.recv_timeout(Duration::from_secs(30))?;
    gtk::run_many(options)?;
    thread.join().map_err(|_| "native source driver panicked")?;
    done_rx.recv()?.map_err(|message| message.into())
}
