//! Socket lifecycle must invalidate the source while its normal owner is stalled.
use rui_host_broker::{ControlledHost, HostConfig, COUNTER_ID};
use rui_protocol_core::{session::*, wire::proto as core};
use rui_transport::{
    pairing::PairingAuthority,
    producer::{
        Connection, ControlledPort, LifecycleFuture, Operation, PortFuture, ProducerPort, Request,
    },
    proto as p, ClientEvent, HostIdentity, NetworkClient, ProducerServerHandle, ServerConfig,
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::Notify;

struct Stalled {
    inner: ControlledPort,
    entered: Notify,
    release: Notify,
    disconnected: Notify,
    pending: AtomicBool,
    stall_ack: AtomicBool,
}
impl ProducerPort for Stalled {
    fn transact(&self, request: Request) -> PortFuture<'_> {
        if matches!(
            &request,
            Request::Scoped {
                operation: Operation::Command { .. },
                ..
            }
        ) || (self.stall_ack.load(Ordering::Acquire)
            && matches!(
                &request,
                Request::Scoped {
                    operation: Operation::Ack(
                        rui_transport::producer::Acknowledgement::Applied { .. }
                    ),
                    ..
                }
            ))
        {
            return Box::pin(async move {
                self.pending.store(true, Ordering::Release);
                self.entered.notify_one();
                self.release.notified().await;
                self.pending.store(false, Ordering::Release);
                self.inner.transact(request).await
            });
        }
        self.inner.transact(request)
    }
    fn disconnect(&self, connection: &Connection) {
        self.inner.disconnect(connection);
        self.disconnected.notify_one();
    }
    fn shutdown(&self) {
        self.inner.shutdown();
    }
    fn flush_lifecycle(&self, now: u64) -> LifecycleFuture<'_> {
        self.inner.flush_lifecycle(now)
    }
}

async fn scenario(mode: u8) {
    let auth = VerifiedAuthorization {
        principal: "stalled-native-owner".into(),
        policy_version: 1,
        permissions: Permissions {
            can_view: true,
            can_control: true,
            actions: ["Activate".into()].into(),
        },
    };
    let mut pairing = PairingAuthority::new(1).unwrap();
    let approval = pairing.request_local_pairing([3; 16]).unwrap();
    let credential = pairing.approve_local(approval.request_id, auth).unwrap();
    let host = Arc::new(tokio::sync::Mutex::new(
        ControlledHost::new(HostConfig::default(), String::new()).unwrap(),
    ));
    let port = Arc::new(Stalled {
        inner: ControlledPort::new(host.clone(), Arc::new(|_| {}), 1).unwrap(),
        entered: Notify::new(),
        release: Notify::new(),
        disconnected: Notify::new(),
        pending: AtomicBool::new(false),
        stall_ack: AtomicBool::new(mode == 3),
    });
    let server = ProducerServerHandle::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        HostIdentity::from_der(
            include_bytes!("data/localhost.der").to_vec(),
            include_bytes!("data/localhost-key.der").to_vec(),
        )
        .unwrap(),
        port.clone(),
        Arc::new(Mutex::new(pairing)),
        ServerConfig {
            max_message_bytes: 4096,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let hello = || {
        let config = SessionConfig::default();
        ClientHello {
            client_instance_id: [3; 16],
            supported_versions: config.versions,
            supported_profiles: config.supported_profiles,
            required_profiles: config.required_profiles,
            receiver_limits: config.receiver_limits,
            resume: None,
        }
    };
    let mut client = NetworkClient::connect(
        server.address,
        "localhost".into(),
        include_bytes!("data/localhost.der").to_vec(),
        &credential,
        hello(),
    )
    .await
    .unwrap();
    client.open_view().await.unwrap();
    while !matches!(
        client.receive().await.unwrap(),
        ClientEvent::Applied { revision: 1 }
    ) {}
    if mode == 3 {
        let records = client.receiver().state().unwrap().records.clone();
        client.flush_pending().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), port.entered.notified())
            .await
            .unwrap();
        let mut retained = client.into_retained();
        tokio::time::timeout(Duration::from_secs(2), port.disconnected.notified())
            .await
            .unwrap();
        assert!(port.pending.load(Ordering::Acquire));
        port.stall_ack.store(false, Ordering::Release);
        let mut recovered = retained
            .reconnect(
                server.address,
                "localhost".into(),
                include_bytes!("data/localhost.der").to_vec(),
                &credential,
                hello(),
            )
            .await
            .unwrap();
        recovered.open_view().await.unwrap();
        let mut reset = false;
        loop {
            match recovered.receive().await.unwrap() {
                ClientEvent::ResetRequired(_) => reset = true,
                ClientEvent::Applied { revision } => {
                    assert_eq!(revision, 2);
                    break;
                }
                ClientEvent::ResumeAccepted(_) => panic!("lost owner ACK must not be invented"),
                ClientEvent::ViewOpened(_) => {}
                other => panic!("unexpected recovery event {other:?}"),
            }
        }
        assert!(reset);
        assert_eq!(recovered.receiver().state().unwrap().records, records);
        assert_eq!(host.lock().await.diagnostics().counter, 0);
        drop(recovered);
        server.shutdown().await;
        return;
    }
    client.presented(1).await.unwrap();
    client.acquire_control([9; 16]).await.unwrap();
    let lease = loop {
        if let ClientEvent::ControlGranted(lease) = client.receive().await.unwrap() {
            break lease;
        }
    };
    client
        .send(
            p::Kind::SemanticCommand,
            &core::SemanticCommand {
                epoch: client.epoch.to_vec(),
                view_id: client.view_id.to_vec(),
                client_instance_id: vec![3; 16],
                command_seq: 1,
                control_lease_id: lease.lease_id,
                control_generation: lease.generation,
                target_id: COUNTER_ID,
                action_id: "Activate".into(),
                action_schema_version: 1,
                observed_view_revision: 1,
                preconditions: Some(core::Preconditions {
                    target_version: Some(1),
                    ..Default::default()
                }),
                arguments: Some(core::semantic_command::Arguments::NoArguments(
                    core::EmptyArguments {},
                )),
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), port.entered.notified())
        .await
        .unwrap();
    match mode {
        0 => drop(client),
        1 => {
            client.close().await.unwrap();
        }
        2 => {
            // Normal ingress saturation must close rather than park its reader
            // behind the stalled source, hiding later lifecycle input.
            for nonce in 0..40 {
                if client
                    .send(p::Kind::Ping, &p::Ping { nonce })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            tokio::time::timeout(Duration::from_secs(2), port.disconnected.notified())
                .await
                .unwrap();
            assert!(port.pending.load(Ordering::Acquire));
            drop(client);
        }
        _ => unreachable!(),
    }
    if mode != 2 {
        tokio::time::timeout(Duration::from_secs(2), port.disconnected.notified())
            .await
            .unwrap();
    }
    assert!(
        port.pending.load(Ordering::Acquire),
        "disconnect must not wait for producer completion"
    );
    assert_eq!(host.lock().await.diagnostics().counter, 0);
    port.inner.flush_lifecycle(0).await.unwrap();
    server.shutdown().await;
}
#[tokio::test]
async fn tls_eof_invalidates_stalled_producer_before_normal_work_finishes() {
    scenario(0).await;
}
#[tokio::test]
async fn websocket_close_invalidates_stalled_producer_before_normal_work_finishes() {
    scenario(1).await;
}
#[tokio::test]
async fn bounded_ingress_overflow_invalidates_stalled_producer() {
    scenario(2).await;
}

#[tokio::test]
async fn loss_of_an_unprocessed_applied_ack_recovers_with_an_explicit_checkpoint_reset() {
    scenario(3).await;
}
