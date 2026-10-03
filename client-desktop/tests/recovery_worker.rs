#![cfg(feature = "desktop")]
//! CPU worker integration with explicit simulated presentation callbacks. Native
//! after-paint and pixel proof is a separate private-display integration gate.
#[path = "../../transport/tests/common/resources.rs"]
#[allow(dead_code)]
mod fixture;
use fixture::*;
use rui_client_desktop::{
    desktop::{self, DesktopObservation, Lifecycle, Options, Stamp, UiCommand, UiEvent, UiState},
    Viewport,
};
use rui_protocol_core::model::Point;
use rui_transport::ServerHandle;
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::mpsc::{self, Receiver, TryRecvError},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc as async_queue, watch};
/// End-to-end TLS remains untouched. This bounded TCP relay withholds encrypted
/// downstream bytes, then closes both legs after the authoritative source has
/// committed. It never inspects credentials or application packets.
struct Relay {
    address: SocketAddr,
    hold: Arc<AtomicBool>,
    initial_byte_barrier: Arc<AtomicUsize>,
    blocked: Arc<AtomicBool>,
    cut: watch::Sender<u64>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Relay {
    async fn new(upstream: SocketAddr) -> Self {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let hold = Arc::new(AtomicBool::new(false));
        let initial_byte_barrier = Arc::new(AtomicUsize::new(0));
        let blocked = Arc::new(AtomicBool::new(false));
        let (cut, _) = watch::channel(0u64);
        let signal = cut.clone();
        let paused = hold.clone();
        let barrier = initial_byte_barrier.clone();
        let downloading_blocked = blocked.clone();
        let task = tokio::spawn(async move {
            let mut children = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    incoming=listener.accept(),if children.len()<4=>{
                        let (down,_)=incoming.unwrap();let pause=paused.clone();let barrier=barrier.clone();let blocked=downloading_blocked.clone();let mut stop=signal.subscribe();stop.borrow_and_update();
                        children.spawn(async move {
                            let up=tokio::net::TcpStream::connect(upstream).await.unwrap();
                            down.set_nodelay(true).unwrap();up.set_nodelay(true).unwrap();
                            let (mut dr,mut dw)=down.into_split();let (mut ur,mut uw)=up.into_split();
                            let upload=async {let mut bytes=[0u8;4096];loop {let n=dr.read(&mut bytes).await?;if n==0 {break;}uw.write_all(&bytes[..n]).await?;}Ok::<(),std::io::Error>(())};
                            let download=async {
                                let mut bytes=[0u8;4096];
                                let mut received=0usize;
                                loop {
                                    let n=ur.read(&mut bytes).await?;
                                    if n==0 {break;}
                                    received+=n;
                                    loop {
                                        let ceiling=barrier.load(Ordering::Acquire);
                                        if !pause.load(Ordering::Acquire)&&(ceiling==0||received<=ceiling) {break;}
                                        blocked.store(true,Ordering::Release);
                                        tokio::time::sleep(Duration::from_millis(1)).await;
                                    }
                                    blocked.store(false,Ordering::Release);
                                    dw.write_all(&bytes[..n]).await?;
                                }
                                Ok::<(),std::io::Error>(())
                            };
                            tokio::select! {_=upload=>{},_=download=>{},_=stop.changed()=>{}}
                        });
                    },
                    _=children.join_next(),if !children.is_empty()=>{},
                }
            }
        });
        Self {
            address,
            hold,
            initial_byte_barrier,
            blocked,
            cut,
            task,
        }
    }
    fn release_and_cut(&self) {
        self.hold.store(false, Ordering::Release);
        self.cut.send_modify(|n| *n += 1);
    }
}
struct Worker {
    commands: async_queue::Sender<UiCommand>,
    events: Receiver<UiEvent>,
    ui: watch::Receiver<UiState>,
    observed: watch::Receiver<DesktopObservation>,
    shutdown: watch::Sender<bool>,
    viewport: watch::Sender<Viewport>,
    budget: Option<rui_protocol_core::resources::MemoryBudget>,
    task: tokio::task::JoinHandle<()>,
}
impl Worker {
    fn start(server: &ServerHandle, credential: rui_transport::pairing::Credential) -> Self {
        Self::start_at(server.address, credential)
    }
    fn start_at(address: SocketAddr, credential: rui_transport::pairing::Credential) -> Self {
        Self::start_with_control(address, credential, true)
    }
    fn start_with_control(
        address: SocketAddr,
        credential: rui_transport::pairing::Credential,
        auto_control: bool,
    ) -> Self {
        let (commands, command_rx) = async_queue::channel(32);
        let (event_tx, events) = mpsc::sync_channel(1);
        let (ui_tx, ui) = watch::channel(UiState::default());
        let (observations, observed) = watch::channel(DesktopObservation::default());
        let (shutdown, shutdown_rx) = watch::channel(false);
        let (_viewport, viewport_rx) = watch::channel(Viewport::new(920, 480));
        let viewport = _viewport.clone();
        let options = Options {
            address,
            server_name: "localhost".into(),
            certificate: include_bytes!("../../transport/tests/data/localhost.der").to_vec(),
            credential,
            client_id: [3; 16],
            view_selection: desktop::ViewSelection::Fixture,
            initial_scale: 1.0,
            auto_control,
            smoke_frames: None,
            title: "worker test",
            observations: Some(observations),
            shutdown_signal: None,
        };
        // Keep the viewport sender alive on the worker's owning task.
        let task = tokio::spawn(async move {
            let _keep = _viewport;
            desktop::worker(
                options,
                command_rx,
                viewport_rx,
                event_tx,
                ui_tx,
                shutdown_rx,
            )
            .await;
        });
        Self {
            commands,
            events,
            ui,
            observed,
            shutdown,
            viewport,
            budget: None,
            task,
        }
    }
    async fn frame(&mut self, min_generation: u64, min_revision: u64) -> Stamp {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            assert!(
                Instant::now() < deadline,
                "worker frame deadline: lifecycle {:?}",
                self.ui.borrow().lifecycle
            );
            match self.events.try_recv() {
                Ok(UiEvent::Frame { stamp, pixels, .. }) => {
                    assert_eq!(pixels.revision, stamp.revision);
                    if stamp.generation >= min_generation
                        && stamp.revision >= min_revision
                        && self.ui.borrow().accepts(stamp)
                    {
                        return stamp;
                    }
                }
                Ok(UiEvent::Ready(budget)) => {
                    assert!(budget.used() <= budget.limit());
                    self.budget = Some(budget);
                }
                Ok(UiEvent::Status(_)) | Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => panic!("worker closed before frame"),
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
    async fn present(&self, stamp: Stamp) {
        self.commands
            .send(UiCommand::Presented(stamp))
            .await
            .unwrap();
    }
    async fn control(&mut self) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !self.observed.borrow().has_control {
                self.observed.changed().await.unwrap();
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "control deadline: {:?}, lifecycle {:?}",
                self.observed.borrow(),
                self.ui.borrow().lifecycle
            )
        });
    }
    async fn stop(self) {
        self.shutdown.send_replace(true);
        tokio::time::timeout(Duration::from_secs(3), self.task)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn rgba_allocation_failure_publishes_suspension_and_can_recover_at_smaller_viewport() {
    use rui_host_broker::{ControlledHost, HostConfig};
    use rui_transport::{pairing::PairingAuthority, HostIdentity, ServerConfig};
    let mut pairing = PairingAuthority::new(1).unwrap();
    let request = pairing.request_local_pairing([3; 16]).unwrap();
    let credential = pairing
        .approve_local(request.request_id, auth(3, false))
        .unwrap();
    let server = ServerHandle::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        HostIdentity::from_der(
            include_bytes!("../../transport/tests/data/localhost.der").to_vec(),
            include_bytes!("../../transport/tests/data/localhost-key.der").to_vec(),
        )
        .unwrap(),
        ControlledHost::new(HostConfig::default(), "private test-only value".into()).unwrap(),
        Arc::new(std::sync::Mutex::new(pairing)),
        ServerConfig::default(),
    )
    .await
    .unwrap();
    let mut worker = Worker::start_with_control(server.address, credential, false);
    let first = worker.frame(0, 1).await;
    let budget = worker
        .budget
        .as_ref()
        .expect("Ready precedes Frame")
        .clone();
    // A600×400 linear float frame fits in8MiB; the simultaneously retained
    // RGBA texture cannot. The real shared view budget must refuse that peak.
    let held = budget
        .reserve(budget.limit() - budget.used() - (8 << 20))
        .unwrap();
    worker.viewport.send_replace(Viewport::new(600, 400));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if worker.ui.borrow().lifecycle == Lifecycle::Suspended {
                break;
            }
            match worker.events.try_recv() {
                Ok(UiEvent::Frame { .. }) => panic!("frame escaped failing texture allocation"),
                Ok(_) | Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => panic!("allocation failure closed worker"),
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        worker.observed.borrow().failure_class,
        Some("display-memory")
    );
    assert!(!worker.ui.borrow().accepts(first));
    assert_eq!(
        worker.ui.borrow().render_notice,
        Some("Display allocation limit reached. Reduce the local viewport size.")
    );
    assert!(!worker.observed.borrow().has_control);
    assert!(budget.used() <= budget.limit());
    worker.viewport.send_replace(Viewport::new(100, 100));
    let recovered = worker.frame(first.generation + 1, 1).await;
    assert!(worker.ui.borrow().accepts(recovered));
    assert_eq!(worker.ui.borrow().render_notice, None);
    drop(held);
    worker.stop().await;
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn dirty_viewport_waits_for_delayed_first_publication_without_presenting_revision_zero() {
    let (server, credential, _, _) = setup().await;
    let relay = Relay::new(server.address).await;
    // This large resource-bearing fixture cannot fit in the first8KiB of
    // encrypted downstream traffic. The earlier TLS/WS/ServerHello handshake
    // fits, so the gate deterministically blocks an incomplete first capture.
    relay.initial_byte_barrier.store(8192, Ordering::Release);
    let mut worker = Worker::start_at(relay.address, credential);
    tokio::time::timeout(Duration::from_secs(5), async {
        while worker.observed.borrow().binding.is_none() || !relay.blocked.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    worker.viewport.send_replace(Viewport::new(921, 481));
    let deadline = Instant::now() + Duration::from_millis(150);
    while Instant::now() < deadline {
        assert_eq!(worker.ui.borrow().lifecycle, Lifecycle::Live);
        assert!(!worker.observed.borrow().has_control);
        match worker.events.try_recv() {
            Ok(UiEvent::Frame { stamp, .. }) => panic!(
                "frame before first publication committed: revision{}",
                stamp.revision
            ),
            Ok(UiEvent::Ready(budget)) => assert!(budget.used() <= budget.limit()),
            Ok(UiEvent::Status(_)) | Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => panic!("worker closed at initial-publication gate"),
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    relay.initial_byte_barrier.store(0, Ordering::Release);
    let frame = worker.frame(0, 1).await;
    assert_eq!((frame.viewport.width, frame.viewport.height), (921, 481));
    worker.present(frame).await;
    worker.control().await;
    assert_eq!(worker.observed.borrow().failure_class, None);
    assert_eq!(server.host.lock().await.diagnostics().consumed_commands, 0);
    worker.stop().await;
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn presentation_and_explicit_control_link_loss_preserve_the_retained_worker() {
    let (server, credential, _, _) = setup().await;
    let relay = Relay::new(server.address).await;
    let mut worker = Worker::start_with_control(relay.address, credential, false);
    let mut stamp = worker.frame(0, 1).await;
    let original = worker.observed.borrow().binding.unwrap();
    for transition in 0..3 {
        if transition != 0 {
            worker.present(stamp).await;
            worker.commands.send(UiCommand::Acquire).await.unwrap();
            worker.control().await;
        }
        // Tear down the live encrypted link next to an after-paint/control
        // callback. Either the write or the reader can observe EOF first.
        relay.release_and_cut();
        worker
            .commands
            .send(match transition {
                0 => UiCommand::Presented(stamp),
                1 => UiCommand::Acquire,
                2 => UiCommand::Release,
                _ => unreachable!(),
            })
            .await
            .unwrap();
        let recovered = worker.frame(stamp.generation + 1, 1).await;
        assert!(!worker.observed.borrow().has_control);
        let binding = worker.observed.borrow().binding.unwrap();
        assert_eq!(
            (binding.client, binding.epoch, binding.view),
            (original.client, original.epoch, original.view)
        );
        assert_eq!(server.host.lock().await.diagnostics().consumed_commands, 0);
        // A delayed old callback cannot retire the freshly issued frame.
        worker.present(stamp).await;
        worker.present(recovered).await;
        worker.commands.send(UiCommand::Acquire).await.unwrap();
        worker.control().await;
        stamp = recovered;
    }
    worker.stop().await;
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn ambiguous_effect_before_reply_replays_exact_pending_bytes_once_and_keeps_sequence() {
    let (server, credential, _, _) = setup().await;
    let relay = Relay::new(server.address).await;
    let mut worker = Worker::start_at(relay.address, credential);
    let initial = worker.frame(0, 1).await;
    worker.present(initial).await;
    worker.control().await;
    relay.hold.store(true, Ordering::Release);
    worker
        .commands
        .send(UiCommand::Click {
            stamp: initial,
            point: Point { x: 20.0, y: 20.0 },
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while server.host.lock().await.diagnostics().counter != 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    relay.release_and_cut();
    let recovered = worker.frame(initial.generation + 1, 2).await;
    let diagnostics = server.host.lock().await.diagnostics();
    assert_eq!(diagnostics.counter, 1);
    assert_eq!(diagnostics.consumed_commands, 1);
    assert!(!worker.observed.borrow().has_control);
    worker.present(recovered).await;
    worker.control().await;
    worker
        .commands
        .send(UiCommand::Click {
            stamp: recovered,
            point: Point { x: 20.0, y: 20.0 },
        })
        .await
        .unwrap();
    let next = worker.frame(recovered.generation, 3).await;
    worker.present(next).await;
    let diagnostics = server.host.lock().await.diagnostics();
    assert_eq!(diagnostics.counter, 2);
    assert_eq!(diagnostics.consumed_commands, 2);
    worker.stop().await;
    server.shutdown().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn retained_worker_reconnect_and_cache_loss_require_fresh_frames_and_drop_stale_input() {
    let (server, credential, _, _) = setup().await;
    let mut worker = Worker::start(&server, credential);
    let initial = worker.frame(0, 1).await;
    worker.present(initial).await;
    worker.control().await;
    worker
        .commands
        .send(UiCommand::Click {
            stamp: initial,
            point: Point { x: 20.0, y: 20.0 },
        })
        .await
        .unwrap();
    let applied = worker.frame(0, 2).await;
    worker.present(applied).await;
    assert_eq!(server.host.lock().await.diagnostics().counter, 1);
    let old_binding = worker.observed.borrow().binding.unwrap();
    worker.commands.send(UiCommand::Reconnect).await.unwrap();
    // A delayed callback from the old widget generation cannot restore a
    // presentation marker or execute another remote action after reconnection.
    worker
        .commands
        .send(UiCommand::Presented(applied))
        .await
        .unwrap();
    worker
        .commands
        .send(UiCommand::Click {
            stamp: applied,
            point: Point { x: 20.0, y: 20.0 },
        })
        .await
        .unwrap();
    let resumed = worker.frame(applied.generation + 1, 2).await;
    assert!(!worker.observed.borrow().has_control);
    assert_eq!(
        worker.observed.borrow().binding.unwrap().view,
        old_binding.view
    );
    assert_eq!(server.host.lock().await.diagnostics().counter, 1);
    worker.present(resumed).await;
    worker.control().await;
    worker
        .commands
        .send(UiCommand::Click {
            stamp: resumed,
            point: Point { x: 20.0, y: 20.0 },
        })
        .await
        .unwrap();
    let next = worker.frame(resumed.generation, 3).await;
    worker.present(next).await;
    assert_eq!(server.host.lock().await.diagnostics().counter, 2);
    worker
        .commands
        .send(UiCommand::LoseResources)
        .await
        .unwrap();
    let restored = worker.frame(next.generation + 1, 3).await;
    assert_eq!(worker.ui.borrow().lifecycle, Lifecycle::Live);
    worker.present(restored).await;
    worker
        .commands
        .send(UiCommand::Click {
            stamp: next,
            point: Point { x: 20.0, y: 20.0 },
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(server.host.lock().await.diagnostics().counter, 2);
    worker.stop().await;
    server.shutdown().await;
}
