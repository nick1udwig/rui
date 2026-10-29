//! A delayed OpenView must not let the heartbeat pump an unopened producer.
use rui_host_broker::{ControlledHost, HostConfig};
use rui_protocol_core::session::{ClientHello, Permissions, SessionConfig, VerifiedAuthorization};
use rui_transport::{
    pairing::PairingAuthority,
    producer::{
        Connection, ControlledPort, LifecycleFuture, Operation, PortFuture, ProducerPort, Request,
    },
    ClientEvent, HostIdentity, NetworkClient, ProducerServerHandle, ServerConfig, TransportError,
};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

struct OpenRequired {
    inner: ControlledPort,
    opened: AtomicBool,
    premature: AtomicUsize,
    pumps: AtomicUsize,
    disconnected: AtomicBool,
}
impl ProducerPort for OpenRequired {
    fn transact(&self, request: Request) -> PortFuture<'_> {
        let opening = matches!(
            &request,
            Request::Scoped {
                operation: Operation::Open { .. },
                ..
            }
        );
        if matches!(
            &request,
            Request::Scoped {
                operation: Operation::Pump { .. },
                ..
            }
        ) {
            if !self.opened.load(Ordering::Acquire) {
                self.premature.fetch_add(1, Ordering::AcqRel);
                return Box::pin(std::future::ready(Err(TransportError::Protocol)));
            }
            self.pumps.fetch_add(1, Ordering::AcqRel);
        }
        Box::pin(async move {
            let reply = self.inner.transact(request).await?;
            if opening {
                self.opened.store(true, Ordering::Release);
            }
            Ok(reply)
        })
    }
    fn disconnect(&self, connection: &Connection) {
        self.disconnected.store(true, Ordering::Release);
        self.inner.disconnect(connection);
    }
    fn shutdown(&self) {
        self.inner.shutdown();
    }
    fn flush_lifecycle(&self, now: u64) -> LifecycleFuture<'_> {
        self.inner.flush_lifecycle(now)
    }
}

#[tokio::test]
async fn heartbeat_waits_for_explicit_open_before_pumping_native_style_producer() {
    let mut pairing = PairingAuthority::new(1).unwrap();
    let request = pairing.request_local_pairing([3; 16]).unwrap();
    let credential = pairing
        .approve_local(
            request.request_id,
            VerifiedAuthorization {
                principal: "delayed-open".into(),
                policy_version: 1,
                permissions: Permissions {
                    can_view: true,
                    can_control: false,
                    actions: Default::default(),
                },
            },
        )
        .unwrap();
    let port = Arc::new(OpenRequired {
        inner: ControlledPort::new(
            Arc::new(tokio::sync::Mutex::new(
                ControlledHost::new(HostConfig::default(), String::new()).unwrap(),
            )),
            Arc::new(|_| {}),
            1,
        )
        .unwrap(),
        opened: AtomicBool::new(false),
        premature: AtomicUsize::new(0),
        pumps: AtomicUsize::new(0),
        disconnected: AtomicBool::new(false),
    });
    let server = ProducerServerHandle::bind(
        "127.0.0.1:0".parse().unwrap(),
        HostIdentity::from_der(
            include_bytes!("data/localhost.der").to_vec(),
            include_bytes!("data/localhost-key.der").to_vec(),
        )
        .unwrap(),
        port.clone(),
        Arc::new(Mutex::new(pairing)),
        ServerConfig::default(),
    )
    .await
    .unwrap();
    let config = SessionConfig::default();
    let mut client = NetworkClient::connect(
        server.address,
        "localhost".into(),
        include_bytes!("data/localhost.der").to_vec(),
        &credential,
        ClientHello {
            client_instance_id: [3; 16],
            supported_versions: config.versions,
            supported_profiles: config.supported_profiles,
            required_profiles: config.required_profiles,
            receiver_limits: config.receiver_limits,
            resume: None,
        },
    )
    .await
    .unwrap();
    // Authenticated ServerHello is received, but no OpenView has been sent.
    // Exercise multiple real heartbeat ticks without relying on a lucky race.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(port.premature.load(Ordering::Acquire), 0);
    assert!(!port.disconnected.load(Ordering::Acquire));
    client.open_view().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if matches!(
                client.receive().await.unwrap(),
                ClientEvent::Applied { revision: 1 }
            ) {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert!(port.opened.load(Ordering::Acquire));
    assert!(port.pumps.load(Ordering::Acquire) > 0);
    assert_eq!(port.premature.load(Ordering::Acquire), 0);
    let pumps = port.pumps.load(Ordering::Acquire);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        port.pumps.load(Ordering::Acquire) - pumps < 25,
        "an idle view must not poll its producer at 500 Hz"
    );
    drop(client);
    server.shutdown().await;
}
