//! Explicit selections stay exact through TLS; rejected scopes are not aliased.
use rui_host_broker::{ControlledHost, HostConfig};
use rui_protocol_core::session::{ClientHello, Permissions, SessionConfig, VerifiedAuthorization};
use rui_transport::{
    pairing::PairingAuthority,
    producer::{
        Connection, ControlledPort, LifecycleFuture, Operation, PortFuture, ProducerPort, Request,
    },
    HostIdentity, NetworkClient, ProducerServerHandle, ServerConfig,
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};
struct Observed {
    inner: ControlledPort,
    selection: Mutex<Option<(String, String)>>,
}
impl ProducerPort for Observed {
    fn transact(&self, request: Request) -> PortFuture<'_> {
        if let Request::Scoped {
            operation:
                Operation::Open {
                    selection,
                    mirror_canvas_policy,
                },
            ..
        } = &request
        {
            *self.selection.lock().unwrap() =
                Some((selection.clone(), mirror_canvas_policy.clone()));
        }
        self.inner.transact(request)
    }
    fn disconnect(&self, connection: &Connection) {
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
async fn explicit_request_is_exact_and_never_widens_to_fixture_scope() {
    let config = SessionConfig::default();
    let authorization = VerifiedAuthorization {
        principal: "selection-test".into(),
        policy_version: 1,
        permissions: Permissions {
            can_view: true,
            can_control: false,
            actions: Default::default(),
        },
    };
    let mut pairing = PairingAuthority::new(1).unwrap();
    let request = pairing.request_local_pairing([3; 16]).unwrap();
    let credential = pairing
        .approve_local(request.request_id, authorization)
        .unwrap();
    let host = Arc::new(tokio::sync::Mutex::new(
        ControlledHost::new(HostConfig::default(), String::new()).unwrap(),
    ));
    let port = Arc::new(Observed {
        inner: ControlledPort::new(host.clone(), Arc::new(|_| {}), 1).unwrap(),
        selection: Mutex::new(None),
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
        ServerConfig::default(),
    )
    .await
    .unwrap();
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
    client
        .open_view_with("session", "host-canvas")
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), client.receive())
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(
        *port.selection.lock().unwrap(),
        Some(("session".into(), "host-canvas".into()))
    );
    let client_identity = rui_protocol_core::commands::AuthenticatedClient {
        principal: "selection-test".into(),
        client_instance_id: [3; 16],
    };
    assert!(host
        .lock()
        .await
        .view_status(&client_identity, client.view_id)
        .unwrap()
        .revision
        .is_none());
    assert_eq!(client.receiver().state().unwrap().revision, 0);
    server.shutdown().await;
}
