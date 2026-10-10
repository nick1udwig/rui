//! Manual visible GTK source and real TLS mirror, locally or on two machines.
use rui_client_desktop::desktop::{gtk, DesktopObservation, Options, ViewSelection};
use rui_producer_sdk::gtk_host::{GtkCommandHost, GtkProducerPort};
use rui_protocol_core::session::{Permissions, VerifiedAuthorization};
use rui_transport::{
    pairing::PairingAuthority, producer::ProducerPort, HostIdentity, ProducerServerHandle,
    ServerConfig, TransportError,
};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::SocketAddr,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

type Error = Box<dyn std::error::Error + Send + Sync>;
const CLIENT_ID: [u8; 16] = [3; 16];
const CLIENT_HEX: &str = "03030303030303030303030303030303";
const USAGE: &str =
    "usage: rui-demo [--host-only] [--listen ADDRESS:PORT] [--identity-dir NEW_DIRECTORY] [--browser-origin EXACT_ORIGIN]";

struct Config {
    host_only: bool,
    browser_origins: Vec<String>,
    address: SocketAddr,
    directory: PathBuf,
}
impl Config {
    fn parse() -> Result<Option<Self>, Error> {
        let mut host_only = false;
        let mut browser_origins = Vec::new();
        let mut address = "127.0.0.1:0".parse()?;
        let mut directory = None;
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--help" | "-h" => {
                    println!("{USAGE}\nOpens a real GTK host and client. --host-only serves the visible host for a separate rui-mirror client.\nThe identity directory must be new and is removed when the demo exits.");
                    return Ok(None);
                }
                "--host-only" => host_only = true,
                "--browser-origin" => browser_origins.push(args.next().ok_or(USAGE)?),
                "--listen" => address = args.next().ok_or(USAGE)?.parse()?,
                "--identity-dir" => directory = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
                _ => return Err(USAGE.into()),
            }
        }
        let directory = directory.unwrap_or_else(|| {
            std::env::temp_dir().join(format!(
                "rui-visual-demo-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ))
        });
        Ok(Some(Self {
            host_only,
            browser_origins,
            address,
            directory,
        }))
    }
}

struct PrivateDirectory(PathBuf);
impl PrivateDirectory {
    fn create(path: &Path) -> Result<Self, Error> {
        fs::DirBuilder::new().mode(0o700).create(path)?;
        Ok(Self(path.canonicalize()?))
    }
}
impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn openssl(args: &[&str], directory: &Path) -> Result<(), Error> {
    let output = Command::new("openssl")
        .args(args)
        .current_dir(directory)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "demo TLS identity generation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}
fn identity(directory: &Path) -> Result<HostIdentity, Error> {
    openssl(
        &[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "7",
            "-subj",
            "/CN=rui-demo",
            "-addext",
            "subjectAltName=DNS:rui-demo",
            "-addext",
            "basicConstraints=critical,CA:FALSE",
            "-addext",
            "keyUsage=critical,digitalSignature,keyEncipherment",
            "-addext",
            "extendedKeyUsage=serverAuth",
            "-keyout",
            "host-key.pem",
            "-out",
            "host.pem",
        ],
        directory,
    )?;
    openssl(
        &[
            "x509", "-in", "host.pem", "-outform", "DER", "-out", "host.der",
        ],
        directory,
    )?;
    openssl(
        &[
            "pkcs8",
            "-topk8",
            "-nocrypt",
            "-in",
            "host-key.pem",
            "-outform",
            "DER",
            "-out",
            "host-key.der",
        ],
        directory,
    )?;
    fs::set_permissions(
        directory.join("host-key.der"),
        fs::Permissions::from_mode(0o600),
    )?;
    Ok(HostIdentity::from_der(
        fs::read(directory.join("host.der"))?,
        fs::read(directory.join("host-key.der"))?,
    )?)
}

struct Ready {
    options: Options,
    stop: watch::Sender<bool>,
}

async fn serve(config: Config, ready: mpsc::SyncSender<Ready>) -> Result<(), Error> {
    let directory = PrivateDirectory::create(&config.directory)?;
    let identity = identity(&directory.0)?;
    let certificate = identity.pinned_certificate();
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let source_directory = directory.0.join("source");
    let mut source = Command::new("python3");
    source
        .arg(repository.join("demos/gtk_host.py"))
        .arg("--output")
        .arg(&source_directory);
    let host = GtkCommandHost::spawn(source, &source_directory)?;
    let port = Arc::new(GtkProducerPort::new(host)?);
    let mut pairing = PairingAuthority::new(1)?;
    let request = pairing.request_local_pairing(CLIENT_ID)?;
    let credential = pairing.approve_local(
        request.request_id,
        VerifiedAuthorization {
            principal: "visual-demo-controller".into(),
            policy_version: 1,
            permissions: Permissions {
                can_view: true,
                can_control: true,
                // Use queued semantic edits for the manual playground. The
                // native wire composition scenarios keep their separate tests.
                actions: ["Activate", "Focus", "ReplaceText", "SetSelection"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            },
        },
    )?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(directory.0.join("client.credential"))?
        .write_all(credential.expose_for_tls_authentication())?;
    let server = ProducerServerHandle::bind(
        config.address,
        identity,
        port.clone(),
        Arc::new(Mutex::new(pairing)),
        ServerConfig {
            max_message_bytes: 128 << 10,
            allowed_browser_origins: config.browser_origins,
            ..ServerConfig::default()
        },
    )
    .await?;
    let (stop, mut stop_rx) = watch::channel(false);
    let (observations, observed) = watch::channel(DesktopObservation::default());
    println!("Visible GTK host listening at {}", server.address);
    println!(
        "Client files: {} and {}",
        directory.0.join("host.der").display(),
        directory.0.join("client.credential").display()
    );
    if config.host_only {
        println!("Copy those two files to the client machine. Connect with TLS name rui-demo and client ID {CLIENT_HEX}. Keep private keys on the host.");
        println!("Close the host window or press Enter here to stop.");
    } else {
        println!("Opening RUI Client. Click Increment host counter or edit text in either window; both views follow the host's state.");
        println!("Close either window to stop the demo.");
    }
    let address = if server.address.ip().is_unspecified() {
        SocketAddr::new(
            if server.address.is_ipv4() {
                "127.0.0.1".parse()?
            } else {
                "::1".parse()?
            },
            server.address.port(),
        )
    } else {
        server.address
    };
    ready
        .send(Ready {
            options: Options {
                address,
                server_name: "rui-demo".into(),
                certificate,
                credential,
                client_id: CLIENT_ID,
                view_selection: ViewSelection::Fixture,
                initial_scale: 0.75,
                auto_control: true,
                smoke_frames: None,
                title: "RUI Client",
                observations: (!config.host_only).then_some(observations),
                shutdown_signal: Some(stop_rx.clone()),
            },
            stop: stop.clone(),
        })
        .map_err(|_| "demo window launcher closed")?;
    let origin = Instant::now();
    let mut refresh = tokio::time::interval(Duration::from_millis(250));
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut client_initialized = config.host_only;
    let result = loop {
        tokio::select! {
            _ = stop_rx.changed() => break Ok(()),
            _ = refresh.tick() => {
                if !client_initialized {
                    let current = *observed.borrow();
                    if current.displayed.is_some() {
                        client_initialized = true;
                    } else if let Some(failure) = current.failure_class {
                        break Err(format!("demo client failed: {failure}").into());
                    }
                    if !client_initialized && origin.elapsed() > Duration::from_secs(15) {
                        break Err(format!("demo client did not display its initial view: {current:?}").into());
                    }
                }
                let now = u64::try_from(origin.elapsed().as_millis()).unwrap_or(u64::MAX);
                match port.refresh_if_changed(now).await {
                    Ok(_) => {},
                    Err(TransportError::Closed) => break Ok(()),
                    Err(error) => break Err(error.into()),
                }
            }
        }
    };
    if let Err(error) = &result {
        eprintln!("Visual demo stopped: {error}");
    }
    stop.send_replace(true);
    server.shutdown().await;
    port.shutdown();
    port.flush_lifecycle(0).await?;
    drop(port);
    if let Ok(path) = std::env::var("RUI_TIMINGS_REPORT") {
        if let Some(report) = rui_protocol_core::timing::inspect(serde_json::to_vec) {
            fs::write(path, report?)?;
        }
    }
    result
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(config) = Config::parse().map_err(|error| -> Box<dyn std::error::Error> { error })?
    else {
        return Ok(());
    };
    // The cooperating exporter currently supports the pinned X11 GTK backend.
    for (name, value) in [
        ("GDK_BACKEND", "x11"),
        ("GSK_RENDERER", "cairo"),
        ("GTK_IM_MODULE", "simple"),
        ("GTK_A11Y", "none"),
        ("GTK_USE_PORTAL", "0"),
        ("GSETTINGS_BACKEND", "memory"),
    ] {
        std::env::set_var(name, value);
    }
    let host_only = config.host_only;
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(3)
            .enable_all()
            .build()
            .map_err(|error| -> Error { error.into() })?;
        runtime.block_on(serve(config, ready_tx))
    });
    let ready = match ready_rx.recv() {
        Ok(ready) => ready,
        Err(_) => {
            thread
                .join()
                .map_err(|_| "demo host thread panicked")?
                .map_err(|error| -> Box<dyn std::error::Error> { error })?;
            return Err("demo host stopped before opening the client".into());
        }
    };
    let gui_result = if host_only {
        let stop = ready.stop.clone();
        std::thread::spawn(move || {
            let mut line = String::new();
            if std::io::stdin()
                .read_line(&mut line)
                .is_ok_and(|count| count > 0)
            {
                stop.send_replace(true);
            }
        });
        Ok(())
    } else {
        let result = gtk::run(ready.options);
        ready.stop.send_replace(true);
        result
    };
    thread
        .join()
        .map_err(|_| "demo host thread panicked")?
        .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    gui_result
}
