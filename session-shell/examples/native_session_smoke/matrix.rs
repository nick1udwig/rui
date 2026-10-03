//! Finite opaque network failures for the owned native mirror test only.
//! The relay never decrypts or inspects TLS; only the controller uses it.
use super::*;
use std::{
    net::SocketAddr,
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

struct Alive(Arc<AtomicUsize>);
impl Drop for Alive {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
pub struct Relay {
    pub address: SocketAddr,
    pause: Arc<AtomicBool>,
    cut: watch::Sender<u64>,
    task: tokio::task::JoinHandle<()>,
    connections: Arc<AtomicU64>,
    alive: Arc<AtomicUsize>,
}
impl Relay {
    pub async fn new(upstream: SocketAddr) -> Result<Self, Error> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let pause = Arc::new(AtomicBool::new(false));
        let (cut, _) = watch::channel(0u64);
        let signal = cut.clone();
        let paused = pause.clone();
        let connections = Arc::new(AtomicU64::new(0));
        let accepted = connections.clone();
        let alive = Arc::new(AtomicUsize::new(0));
        let live = alive.clone();
        let task = tokio::spawn(async move {
            let mut children = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    connection=listener.accept(),if children.len()<4=>{
                        let Ok((down,_))=connection else {break};
                        accepted.fetch_add(1,Ordering::AcqRel);
                        let active=live.clone();
                        let pause=paused.clone();let mut stop=signal.subscribe();stop.borrow_and_update();
                        children.spawn(async move {
                            let up=TcpStream::connect(upstream).await?;
                            active.fetch_add(1,Ordering::AcqRel);
                            let _alive=Alive(active);
                            down.set_nodelay(true)?;up.set_nodelay(true)?;
                            let (mut dr,mut dw)=down.into_split();let (mut ur,mut uw)=up.into_split();
                            let upload=async {
                                let mut buffer=[0u8;4096];
                                loop {let n=dr.read(&mut buffer).await?;if n==0 {break;}
                                    while pause.load(Ordering::Acquire){tokio::time::sleep(Duration::from_millis(1)).await;}
                                    uw.write_all(&buffer[..n]).await?;
                                } Ok::<(),std::io::Error>(())
                            };
                            let download=async {let mut buffer=[0u8;4096];loop {let n=ur.read(&mut buffer).await?;if n==0 {break;}dw.write_all(&buffer[..n]).await?;}Ok::<(),std::io::Error>(())};
                            tokio::select! {_=upload=>{},_=download=>{},_=stop.changed()=>{}}
                            // An opaque pause also delays propagation of EOF.
                            // This allows a real local callback to win authority
                            // before a mirror's independent focus-loss stop.
                            while pause.load(Ordering::Acquire) {
                                tokio::select! {
                                    _=tokio::time::sleep(Duration::from_millis(1))=>{},
                                    _=stop.changed()=>{break;}
                                }
                            }
                            Ok::<(),std::io::Error>(())
                        });
                    },
                    _=children.join_next(),if !children.is_empty()=>{},
                }
            }
        });
        Ok(Self {
            address,
            pause,
            cut,
            task,
            connections,
            alive,
        })
    }
    pub fn cut(&self) {
        self.pause.store(false, Ordering::Release);
        self.cut.send_modify(|v| *v += 1);
    }
    pub fn pause_upload(&self) {
        self.pause.store(true, Ordering::Release);
    }
    pub fn resume_upload(&self) {
        self.pause.store(false, Ordering::Release);
    }
}
impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Disconnect,
    Expiry,
    Revoke,
    LocalTakeover,
}
impl Mode {
    pub fn parse(value: &str) -> Result<Self, Error> {
        Ok(match value {
            "disconnect" => Self::Disconnect,
            "expiry" => Self::Expiry,
            "revoke" => Self::Revoke,
            "local-takeover" => Self::LocalTakeover,
            _ => return Err("unknown bounded native cleanup case".into()),
        })
    }
    fn name(self) -> &'static str {
        match self {
            Self::Disconnect => "disconnect",
            Self::Expiry => "expiry",
            Self::Revoke => "revoke",
            Self::LocalTakeover => "local-takeover",
        }
    }
}
pub struct Context {
    pub mode: Mode,
    pub relay: Relay,
    pub pairing: Arc<Mutex<PairingAuthority>>,
    pub credential: rui_transport::pairing::Credential,
}
fn button(root: &Path, pressed: bool, position: &[String; 2]) -> Result<(), Error> {
    let result = Command::new("/usr/bin/python3")
        .arg(root.join("fixtures/session-shell/native_held.py"))
        .arg(if pressed { "down" } else { "up" })
        .args(position)
        .output()?;
    if !result.status.success() {
        return Err("private held-button transition failed".into());
    }
    Ok(())
}
async fn owner_ready(host: &HostPort) -> Result<(), Error> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let pending = host.status()?;
        let value = loop {
            if let Some(response) = pending.try_recv()? {
                break response?;
            }
            if Instant::now() >= deadline {
                return Err("native owner reply deadline".into());
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        };
        match value.value() {
            Response::Status(rui_session_shell::actor::OwnerStatus::Ready) => return Ok(()),
            Response::Status(rui_session_shell::actor::OwnerStatus::Cleaning) => {}
            _ => return Err("native owner did not remain healthy".into()),
        }
        if Instant::now() >= deadline {
            return Err("native STOP cleanup deadline".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
pub(super) async fn settle_source_focus(
    host: &HostPort,
    local: &watch::Receiver<LocalObservation>,
) -> Result<(), Error> {
    let after = glib::monotonic_time();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let state = *local.borrow();
        if state.failed {
            return Err("source frontend failed during focus barrier".into());
        }
        if state.sampled_at_us > after && !state.active {
            break;
        }
        if Instant::now() >= deadline {
            return Err("actual source focus-loss barrier deadline".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    owner_ready(host).await
}
pub struct Run<'a> {
    pub port: &'a NativeProducerPort,
    pub host: &'a HostPort,
    pub root: &'a Path,
    pub observations: &'a [watch::Receiver<DesktopObservation>],
    pub local: &'a watch::Receiver<LocalObservation>,
    pub before: &'a Compared,
    pub context: &'a Context,
    pub report: Option<&'a Path>,
}
pub async fn run(input: Run<'_>) -> Result<(), Error> {
    let Run {
        port,
        host,
        root,
        observations,
        local,
        before,
        context,
        report,
    } = input;
    let click = Point { x: 30., y: 64. };
    let position = point(before, click);
    let initial = diagnostics(host).await?;
    button(root, true, &position)?;
    held_key(root, true)?;
    let deadline = Instant::now() + Duration::from_secs(4);
    let held = loop {
        let d = diagnostics(host).await?;
        if d.held_keys == 1 && d.held_buttons == 1 {
            break d;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "actual source never held both inputs: keys={} buttons={} frames={} mirror={:?}",
                d.held_keys,
                d.held_buttons,
                d.native_frames_applied,
                observations[0].borrow().physical_notice
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert!(held.native_frames_applied >= initial.native_frames_applied + 2);
    let original_lease = held
        .lease
        .ok_or("missing source authority for verified held input")?;
    let local_before = local.borrow().callbacks[2];
    // Hold ordinary native input at the source BEFORE the trigger. No synthetic
    // held flags or test-only broker releases can satisfy this gate.
    eprintln!(
        "native held matrix: {} with verified source key+button",
        context.mode.name()
    );
    match context.mode {
        Mode::Disconnect => context.relay.cut(),
        Mode::Expiry => context.relay.pause_upload(),
        Mode::Revoke => {
            context
                .pairing
                .lock()
                .map_err(|_| "pairing mutex")?
                .revoke_local([81; 16]);
        }
        Mode::LocalTakeover => {
            context.relay.pause_upload();
            let o = *local.borrow();
            let scale = (f64::from(o.width) / 200.).min(f64::from(o.height) / 120.);
            let p = [
                (o.origin.x + (f64::from(o.width) - 200. * scale) / 2. + 30. * scale).to_string(),
                (o.origin.y + (f64::from(o.height) - 120. * scale) / 2. + 64. * scale).to_string(),
            ];
            native(root, SOURCE, "click", &p)?;
        }
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let cleaned = loop {
        let d = diagnostics(host).await?;
        if d.held_keys == 0 && d.held_buttons == 0 && d.held_touches == 0 {
            break d;
        }
        if Instant::now() >= deadline {
            return Err("trigger did not neutralize actual native source input".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert!(
        cleaned.native_frames_cancelled >= held.native_frames_cancelled + 2,
        "actual EIS key and button release frames are required"
    );
    eprintln!("native held matrix: native releases observed; waiting STOP/Ready");
    owner_ready(host).await?;
    let final_authority = diagnostics(host).await?;
    let cleanup = final_authority
        .last_cleanup
        .as_ref()
        .ok_or("missing authoritative cleanup cause")?;
    assert_eq!(cleanup.generation, original_lease.generation);
    assert!(final_authority.lease.is_none());
    use rui_protocol_core::session::CleanupReason;
    match context.mode {
        Mode::Disconnect => assert_eq!(cleanup.reason, CleanupReason::Disconnect),
        Mode::Expiry => {
            assert_eq!(cleanup.reason, CleanupReason::Expired);
            assert!(cleanup.at_ms >= original_lease.deadline_ms);
            assert_eq!(
                context.relay.connections.load(Ordering::Acquire),
                1,
                "expiry replaced original TLS connection"
            );
            assert_eq!(
                context.relay.alive.load(Ordering::Acquire),
                1,
                "expiry closed original TLS connection"
            );
            assert!(
                context
                    .pairing
                    .lock()
                    .map_err(|_| "pairing mutex")?
                    .authenticated(context.credential.expose_for_tls_authentication(), [81; 16])
                    .is_ok(),
                "expiry revoked approval"
            );
        }
        Mode::Revoke => assert_eq!(cleanup.reason, CleanupReason::AuthorizationChanged),
        Mode::LocalTakeover => {
            assert_eq!(cleanup.reason, CleanupReason::LocalActivity);
            assert!(final_authority.native_work_generation > held.native_work_generation);
            let deadline = Instant::now() + Duration::from_secs(3);
            while local.borrow().callbacks[2] <= local_before {
                if Instant::now() >= deadline {
                    return Err("actual local GDK callback missing at takeover".into());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
    eprintln!("native held matrix: authoritative cleanup cause confirmed");
    context.relay.resume_upload();
    // Old local ups belong to the cleared capture. They must not become new
    // source input after the authority has already performed native cleanup.
    held_key(root, false)?;
    button(root, false, &position)?;
    eprintln!("native held matrix: original physical ups delivered");
    wait(&observations[0], |o| !o.has_control).await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let after_ups = diagnostics(host).await?;
    assert_eq!(
        after_ups.native_frames_applied, cleaned.native_frames_applied,
        "late old-generation callbacks caused a new native effect"
    );
    assert_eq!(
        (
            after_ups.held_keys,
            after_ups.held_buttons,
            after_ups.held_touches
        ),
        (0, 0, 0)
    );
    assert!(
        !observations[0].borrow().has_control,
        "cleanup cannot silently reacquire control"
    );
    eprintln!("native held matrix: checking healthy observer");
    let observer = compare(port, &observations[1]).await?;
    assert!(
        !observations[1].borrow().closed,
        "healthy observer was affected by controller fault"
    );
    if context.mode == Mode::Revoke {
        let binding = observations[0].borrow().binding.unwrap();
        assert!(
            port.projected_source(binding.client, binding.view)
                .await
                .is_err(),
            "revocation still allowed new projection reads"
        );
    } else {
        wait(&observations[0], |o| {
            o.displayed.is_some() && !o.has_control
        })
        .await?;
        let recovered = compare(port, &observations[0]).await?;
        native(root, CONTROLLER, "click", &point(&recovered, click))?;
        settle_source_focus(host, local).await?;
        eprintln!("native held matrix: source focus settled; explicit fresh Acquire");
        request_control(port, root, &observations[0]).await?;
        wait(&observations[0], |o| o.has_control && o.physical_ready).await?;
        assert_eq!(
            (
                diagnostics(host).await?.held_keys,
                diagnostics(host).await?.held_buttons
            ),
            (0, 0),
            "fresh explicit grant restored old held state"
        );
        // A new actual press/release proves the source remains usable only
        // after a deliberate fresh grant, not by resurrecting the old lease.
        let d = diagnostics(host).await?;
        held_key(root, true)?;
        let deadline = Instant::now() + Duration::from_secs(3);
        while diagnostics(host).await?.held_keys != 1 {
            if Instant::now() >= deadline {
                return Err("fresh native input unavailable after cleanup".into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        held_key(root, false)?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let next = diagnostics(host).await?;
            if next.held_keys == 0 {
                assert_eq!(next.native_frames_applied, d.native_frames_applied + 2);
                break;
            }
            if Instant::now() >= deadline {
                return Err("fresh native release deadline".into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        pair(port, observations).await?;
    }
    if let Some(report) = report {
        let mut out = File::create(report)?;
        writeln!(out,"{{\"scope\":\"private actual GTK callbacks/TLS/native EIS cleanup, no scanout claim\",\"matrix\":{:?},\"source_file_fingerprints\":{{{}}},\"held_before\":[1,1,0],\"held_after\":[0,0,0],\"actual_cleanup_release_frames\":{},\"original_lease_generation\":{},\"original_deadline_ms\":{},\"cleanup_reason\":{:?},\"cleanup_at_ms\":{},\"original_native_work_generation\":{},\"final_native_work_generation\":{},\"observer_revision\":{},\"observer_records_sha256\":\"{}\",\"observer_after_paint_rgba_sha256\":\"{}\"}}",context.mode.name(),source_hashes(root)?,cleaned.native_frames_cancelled-held.native_frames_cancelled,original_lease.generation,original_lease.deadline_ms,format!("{:?}",cleanup.reason),cleanup.at_ms,held.native_work_generation,final_authority.native_work_generation,observer.capture.state().revision,hex(&observer.records),hex(&observer.pixels))?;
    }
    eprintln!(
        "native held matrix: {} actual cleanup and independent observer passed",
        context.mode.name()
    );
    Ok(())
}
