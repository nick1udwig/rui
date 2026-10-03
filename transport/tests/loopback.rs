use prost::Message;
use rui_host_broker::{
    ControlledHost, HostConfig, COUNTER_ID, READ_SECRET_PERMISSION, SECRET_ID, SECRET_TEXT_ID,
};
use rui_protocol_core::wire::proto as core;
use rui_protocol_core::{
    commands::AuthenticatedClient,
    model::{Record, TextStatus, TypedValue},
    session::*,
};
use rui_transport::{
    codec,
    pairing::{Credential, PairingAuthority},
    proto as p, ClientEvent, HostIdentity, NetworkClient, ServerConfig, ServerHandle,
    TransportError,
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};
const SECRET: &str = "Aé🦀Z-SECRET-NETWORK-SENTINEL";
fn authorization(name: &str, control: bool) -> VerifiedAuthorization {
    VerifiedAuthorization {
        principal: name.into(),
        policy_version: 1,
        permissions: Permissions {
            can_view: true,
            can_control: control,
            actions: if control {
                ["Activate", "ReplaceText", READ_SECRET_PERMISSION]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            } else {
                Default::default()
            },
        },
    }
}
fn hello(id: u8) -> ClientHello {
    ClientHello {
        client_instance_id: [id; 16],
        supported_versions: [1].into_iter().collect(),
        supported_profiles: SessionConfig::default().supported_profiles,
        required_profiles: SessionConfig::default().required_profiles,
        receiver_limits: ReceiverLimits {
            chunk_bytes: 64,
            ..ReceiverLimits::default()
        },
        resume: None,
    }
}
fn identity() -> HostIdentity {
    HostIdentity::from_der(
        include_bytes!("data/localhost.der").to_vec(),
        include_bytes!("data/localhost-key.der").to_vec(),
    )
    .unwrap()
}
fn approve(pairing: &mut PairingAuthority, id: u8, auth: VerifiedAuthorization) -> Credential {
    let request = pairing.request_local_pairing([id; 16]).unwrap();
    pairing.approve_local(request.request_id, auth).unwrap()
}
async fn setup() -> (
    ServerHandle,
    Arc<Mutex<PairingAuthority>>,
    Credential,
    Credential,
) {
    let mut pairing = PairingAuthority::new(4).unwrap();
    let controller = approve(&mut pairing, 3, authorization("controller", true));
    let observer = approve(&mut pairing, 4, authorization("observer", false));
    let pairing = Arc::new(Mutex::new(pairing));
    let host = ControlledHost::new(
        HostConfig {
            max_text_bytes: 256,
            ..HostConfig::default()
        },
        SECRET.into(),
    )
    .unwrap();
    let server = ServerHandle::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        identity(),
        host,
        pairing.clone(),
        ServerConfig {
            max_message_bytes: 4096,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    (server, pairing, controller, observer)
}
async fn connect(server: &ServerHandle, credential: &Credential, id: u8) -> NetworkClient {
    NetworkClient::connect(
        server.address,
        "localhost".into(),
        include_bytes!("data/localhost.der").to_vec(),
        credential,
        hello(id),
    )
    .await
    .unwrap()
}
async fn open(client: &mut NetworkClient) {
    client.open_view().await.unwrap();
    assert!(matches!(
        client.receive().await.unwrap(),
        ClientEvent::ViewOpened(_)
    ));
    assert!(matches!(
        client.receive().await.unwrap(),
        ClientEvent::Applied { revision: 1 }
    ));
    client.presented(1).await.unwrap();
}
async fn lease(client: &mut NetworkClient) -> p::ControlGranted {
    client.acquire_control([9; 16]).await.unwrap();
    match client.receive().await.unwrap() {
        ClientEvent::ControlGranted(lease) => lease,
        event => panic!("unexpected public event: {event:?}"),
    }
}
fn activate(
    client: &NetworkClient,
    id: u8,
    lease: &p::ControlGranted,
    seq: u64,
    version: u64,
) -> core::SemanticCommand {
    core::SemanticCommand {
        epoch: client.epoch.to_vec(),
        view_id: client.view_id.to_vec(),
        client_instance_id: vec![id; 16],
        command_seq: seq,
        control_lease_id: lease.lease_id.clone(),
        control_generation: lease.generation,
        target_id: COUNTER_ID,
        action_id: "Activate".into(),
        action_schema_version: 1,
        observed_view_revision: 1,
        preconditions: Some(core::Preconditions {
            target_version: Some(version),
            ..Default::default()
        }),
        arguments: Some(core::semantic_command::Arguments::NoArguments(
            core::EmptyArguments {},
        )),
    }
}
fn assert_private(client: &NetworkClient) {
    let state = client.receiver().state().unwrap();
    assert!(!format!("{state:?}").contains("SECRET-NETWORK-SENTINEL"));
    assert!(
        matches!(&state.records[&SECRET_ID],Record::Semantic(n) if n.value.is_none()&&n.actions.is_empty())
    );
    assert!(
        matches!(&state.records[&SECRET_TEXT_ID],Record::Text(t) if t.status==TextStatus::Redacted&&t.content.is_none())
    );
}
async fn result_and_revision(client: &mut NetworkClient, revision: u64) -> p::CommandResult {
    let mut result = None;
    let mut applied = false;
    for _ in 0..8 {
        match client.receive().await.unwrap() {
            ClientEvent::CommandResult(r) => result = Some(r),
            ClientEvent::Applied { revision: r } if r == revision => applied = true,
            _ => {}
        }
        if applied {
            if let Some(result) = result.take() {
                return result;
            }
        }
    }
    panic!("bounded event count exceeded")
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pinned_tls_two_clients_publish_once_and_edit_unicode_at_source() {
    let (server, _pairing, controller_token, observer_token) = setup().await;
    let mut controller = connect(&server, &controller_token, 3).await;
    let mut observer = connect(&server, &observer_token, 4).await;
    assert_eq!(controller.limits.envelope_bytes, 4096);
    assert_eq!(controller.limits.chunk_bytes, 64);
    open(&mut controller).await;
    open(&mut observer).await;
    assert_private(&observer);
    let lease = lease(&mut controller).await;
    let command = activate(&controller, 3, &lease, 1, 1).encode_to_vec();
    controller.submit(&command).await.unwrap();
    let result = result_and_revision(&mut controller, 2).await;
    assert_eq!(result.status, "Applied");
    assert_eq!(result.source_commit, "fixture-commit-2");
    assert!(matches!(
        observer.receive().await.unwrap(),
        ClientEvent::Applied { revision: 2 }
    ));
    assert_private(&observer);
    assert!(
        matches!(&observer.receiver().state().unwrap().records[&COUNTER_ID],Record::Semantic(n) if n.value==Some(TypedValue::Integer(1)))
    );
    controller.submit(&command).await.unwrap();
    let duplicate = match controller.receive().await.unwrap() {
        ClientEvent::CommandResult(r) => r,
        _ => panic!(),
    };
    assert_eq!(result, duplicate);
    let mut edit = activate(&controller, 3, &lease, 2, 1);
    edit.target_id = SECRET_ID;
    edit.action_id = "ReplaceText".into();
    edit.preconditions = Some(core::Preconditions {
        target_version: Some(1),
        text_version: Some(1),
        focus_generation: Some(1),
        ..Default::default()
    });
    edit.arguments = Some(core::semantic_command::Arguments::ReplaceText(
        core::TextArguments {
            start: 1,
            end: 3,
            text: "ß".into(),
        },
    ));
    controller.submit(&edit.encode_to_vec()).await.unwrap();
    let edit_result = result_and_revision(&mut controller, 3).await;
    assert_eq!(edit_result.status, "Applied");
    assert!(
        matches!(&controller.receiver().state().unwrap().records[&SECRET_TEXT_ID],Record::Text(t) if t.content.as_deref()==Some("AßZ-SECRET-NETWORK-SENTINEL"))
    );
    assert!(matches!(
        observer.receive().await.unwrap(),
        ClientEvent::Applied { revision: 3 }
    ));
    assert_private(&observer);
    let observer_attempt = activate(&observer, 4, &lease, 1, 2).encode_to_vec();
    observer.submit(&observer_attempt).await.unwrap();
    assert!(
        matches!(observer.receive().await.unwrap(),ClientEvent::CommandResult(r) if r.status=="Rejected"&&r.source_commit.is_empty())
    );
    observer
        .send(
            p::Kind::NeedResources,
            &p::ResourceIds {
                digests: vec![vec![1; 32]],
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(observer.receive().await.unwrap(),ClientEvent::Error(code) if code=="AUTHORITY_REJECTED")
    );
    let diagnostics = server.host.lock().await.diagnostics();
    assert_eq!(diagnostics.counter, 1);
    assert_eq!(diagnostics.source_commit, 3);
    assert_eq!(diagnostics.consumed_commands, 3);
    assert!(
        !format!("{diagnostics:?} {result:?} {edit_result:?}").contains("SECRET-NETWORK-SENTINEL")
    );
    let observer_identity = AuthenticatedClient {
        principal: "observer".into(),
        client_instance_id: [4; 16],
    };
    let host = server.host.lock().await;
    let snapshot = host.snapshot(&observer_identity, observer.view_id).unwrap();
    assert!(!snapshot
        .windows(SECRET.len())
        .any(|w| w == SECRET.as_bytes()));
    assert!(
        host.retained_memory_bytes(&observer_identity, observer.view_id)
            .unwrap()
            <= observer.limits.view_memory_bytes
    );
    drop(host);
    drop(snapshot);
    drop(controller);
    drop(observer);
    server.shutdown().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn credentials_pin_name_and_binding_fail_before_projection() {
    let (server, _pairing, token, _) = setup().await;
    let bad = Credential::from_bytes([0; 32]);
    assert!(NetworkClient::connect(
        server.address,
        "localhost".into(),
        include_bytes!("data/localhost.der").to_vec(),
        &bad,
        hello(3)
    )
    .await
    .is_err());
    assert!(NetworkClient::connect(
        server.address,
        "localhost".into(),
        include_bytes!("data/localhost.der").to_vec(),
        &token,
        hello(4)
    )
    .await
    .is_err());
    assert!(matches!(
        NetworkClient::connect(
            server.address,
            "untrusted.example".into(),
            include_bytes!("data/localhost.der").to_vec(),
            &token,
            hello(3)
        )
        .await,
        Err(TransportError::Tls)
    ));
    let mut pin = include_bytes!("data/localhost.der").to_vec();
    pin[30] ^= 1;
    assert!(
        NetworkClient::connect(server.address, "localhost".into(), pin, &token, hello(3))
            .await
            .is_err()
    );
    assert_eq!(server.host.lock().await.diagnostics().clients, 0);
    let mut client = connect(&server, &token, 3).await;
    open(&mut client).await;
    let wrong = codec::wrap(
        p::Kind::Ping,
        &p::Ping { nonce: 7 },
        Some((client.session_id, client.epoch, [99; 16])),
    );
    client.send_packet(wrong).await.unwrap();
    assert!(client.receive().await.is_err());
    server.shutdown().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoke_then_reapprove_cannot_keep_old_live_permissions() {
    let (server, pairing, token, _) = setup().await;
    let mut client = connect(&server, &token, 3).await;
    open(&mut client).await;
    let _lease = lease(&mut client).await;
    let replacement = {
        let mut authority = pairing.lock().unwrap();
        assert!(authority.revoke_local([3; 16]));
        approve(&mut authority, 3, authorization("controller", false))
    };
    assert!(
        tokio::time::timeout(Duration::from_secs(1), client.receive())
            .await
            .unwrap()
            .is_err()
    );
    drop(client);
    let mut new = connect(&server, &replacement, 3).await;
    open(&mut new).await;
    new.acquire_control([7; 16]).await.unwrap();
    assert!(matches!(
        new.receive().await.unwrap(),
        ClientEvent::Error(_)
    ));
    let mut packet = codec::wrap(
        p::Kind::Ping,
        &p::Ping { nonce: 3 },
        Some((new.session_id, new.epoch, new.view_id)),
    );
    packet.required_features = vec!["unknown-required.v1".into()];
    new.send_packet(packet).await.unwrap();
    assert!(new.receive().await.is_err());
    server.shutdown().await;
}
#[tokio::test]
async fn fresh_receiver_refuses_resume_without_retained_replica() {
    let mut h = hello(3);
    h.resume = Some(ResumeRequest {
        epoch: [2; 16],
        view_id: [3; 16],
        acknowledged_revision: 1,
    });
    assert!(matches!(
        NetworkClient::connect(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 1)),
            "localhost".into(),
            vec![],
            &Credential::from_bytes([0; 32]),
            h
        )
        .await,
        Err(TransportError::Protocol)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_messages_interleave_and_slow_observer_closes_independently() {
    let (server, _pairing, controller_token, observer_token) = setup().await;
    let mut controller = connect(&server, &controller_token, 3).await;
    controller.open_view().await.unwrap();
    assert!(matches!(
        controller.receive().await.unwrap(),
        ClientEvent::ViewOpened(_)
    ));
    controller
        .send(p::Kind::Ping, &p::Ping { nonce: 42 })
        .await
        .unwrap();
    assert!(matches!(
        controller.receive().await.unwrap(),
        ClientEvent::Pong(42)
    ));
    assert!(matches!(
        controller.receive().await.unwrap(),
        ClientEvent::Applied { revision: 1 }
    ));
    controller.presented(1).await.unwrap();
    let mut limits = hello(4);
    limits.receiver_limits.outbound_queue_bytes = 1024;
    let mut observer = NetworkClient::connect(
        server.address,
        "localhost".into(),
        include_bytes!("data/localhost.der").to_vec(),
        &observer_token,
        limits,
    )
    .await
    .unwrap();
    open(&mut observer).await;
    {
        let mut host = server.host.lock().await;
        for index in 0..32 {
            host.set_counter_enabled(index % 2 != 0, index + 1).unwrap();
        }
    }
    let mut closed = false;
    for _ in 0..8 {
        if observer.receive().await.is_err() {
            closed = true;
            break;
        }
    }
    assert!(closed);
    assert_eq!(server.host.lock().await.diagnostics().closed_views, 1);
    for revision in 2..=33 {
        assert!(
            matches!(controller.receive().await.unwrap(),ClientEvent::Applied {revision:r} if r==revision)
        );
    }
    assert_eq!(controller.receiver().state().unwrap().revision, 33);
    controller
        .send(p::Kind::Ping, &p::Ping { nonce: 43 })
        .await
        .unwrap();
    assert!(matches!(
        controller.receive().await.unwrap(),
        ClientEvent::Pong(43)
    ));
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn semantic_only_projection_controls_after_explicit_ready_without_presented() {
    let (server, _, token, _) = setup().await;
    let mut advertised = hello(3);
    advertised
        .supported_profiles
        .insert("semantics.only.v1".into());
    advertised
        .required_profiles
        .insert("semantics.only.v1".into());
    let mut client = NetworkClient::connect(
        server.address,
        "localhost".into(),
        include_bytes!("data/localhost.der").to_vec(),
        &token,
        advertised,
    )
    .await
    .unwrap();
    client.open_view().await.unwrap();
    assert!(
        matches!(client.receive().await.unwrap(),ClientEvent::ViewOpened(v) if v.coverage=="semantic-only")
    );
    assert!(matches!(
        client.receive().await.unwrap(),
        ClientEvent::Applied { revision: 1 }
    ));
    let state = client.receiver().state().unwrap();
    assert!(state
        .records
        .values()
        .all(|r| !matches!(r, Record::Scene(_) | Record::Binding(_))));
    for record in state.records.values() {
        match record {
            Record::Surface(s) => assert!(s.scene_root.is_none()),
            Record::Semantic(s) => assert!(s.bindings.is_empty()),
            Record::Session(s) => assert!(s.cursor.is_none()),
            _ => {}
        }
    }
    assert_eq!(client.presented(1).await, Err(TransportError::Authority));
    client.acquire_control([11; 16]).await.unwrap();
    assert!(matches!(
        client.receive().await.unwrap(),
        ClientEvent::Error(_)
    ));
    client.semantic_ready(1).await.unwrap();
    let lease = lease(&mut client).await;
    let raw = activate(&client, 3, &lease, 1, 1).encode_to_vec();
    client.submit(&raw).await.unwrap();
    let result = result_and_revision(&mut client, 2).await;
    assert_eq!(result.status, "Applied");
    assert_eq!(server.host.lock().await.diagnostics().counter, 1);
    client
        .send(p::Kind::PresentedAck, &p::RevisionAck { revision: 2 })
        .await
        .unwrap();
    assert!(matches!(
        client.receive().await.unwrap(),
        ClientEvent::Error(_)
    ));
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_receive_retains_staging_and_applied_ack_without_losing_event() {
    let (server, _, token, _) = setup().await;
    let mut client = connect(&server, &token, 3).await;
    client.open_view().await.unwrap();
    assert!(matches!(
        client.receive().await.unwrap(),
        ClientEvent::ViewOpened(_)
    ));
    let mut cancellations = 0;
    let mut applied = false;
    for _ in 0..500 {
        match tokio::time::timeout(Duration::from_millis(1), client.receive()).await {
            Err(_) => cancellations += 1,
            Ok(Ok(ClientEvent::Applied { revision: 1 })) => {
                applied = true;
                break;
            }
            other => panic!("unexpected public event: {other:?}"),
        }
    }
    assert!(cancellations > 0 && applied);
    assert_eq!(client.receiver().state().unwrap().revision, 1);
    assert_eq!(client.pending_outbound_packets(), 1);
    // The deterministic duplex unit test exercises an actually polled send
    // under backpressure. Here real TLS presentation flushes the retained ACK.
    client.presented(1).await.unwrap();
    assert_eq!(client.pending_outbound_packets(), 0);
    let lease = lease(&mut client).await;
    let command = activate(&client, 3, &lease, 1, 1).encode_to_vec();
    client.submit(&command).await.unwrap();
    let result = result_and_revision(&mut client, 2).await;
    assert_eq!(result.status, "Applied");
    assert_eq!(server.host.lock().await.diagnostics().counter, 1);
    client
        .send(p::Kind::Ping, &p::Ping { nonce: 44 })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match tokio::time::timeout(Duration::from_millis(1), client.receive()).await {
                Err(_) => {}
                Ok(Ok(ClientEvent::Pong(44))) => break,
                other => panic!("unexpected public event: {other:?}"),
            }
        }
    })
    .await
    .expect("Pong must arrive before deadline");
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_control_retries_preserve_grants_and_old_release_cannot_revoke_new_lease() {
    async fn grant(client: &mut NetworkClient) -> p::ControlGranted {
        for _ in 0..8 {
            match client.receive().await.unwrap() {
                ClientEvent::ControlGranted(grant) => return grant,
                ClientEvent::ControlRevoked(_) => {}
                event => panic!("unexpected lease event: {event:?}"),
            }
        }
        panic!("bounded lease event count exceeded")
    }
    tokio::time::timeout(Duration::from_secs(15), async {
        let (server, _, token, _) = setup().await;
        let mut client = connect(&server, &token, 3).await;
        open(&mut client).await;
        // Discard a real TLS reply at the application boundary, preserving it only
        // as an oracle. The live connection retries the identical request ID.
        client.acquire_control([31; 16]).await.unwrap();
        let lost_acquire = grant(&mut client).await;
        client.acquire_control([31; 16]).await.unwrap();
        assert_eq!(grant(&mut client).await, lost_acquire);
        let renew = p::RenewControl {
            request_id: vec![32; 16],
            lease_id: lost_acquire.lease_id.clone(),
            generation: lost_acquire.generation,
        };
        client.send(p::Kind::RenewControl, &renew).await.unwrap();
        let lost_renew = grant(&mut client).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        client.send(p::Kind::RenewControl, &renew).await.unwrap();
        assert_eq!(grant(&mut client).await, lost_renew);
        assert_eq!(lost_renew.lease_id, lost_acquire.lease_id);
        assert_eq!(lost_renew.generation, lost_acquire.generation);
        let newer_renew = p::RenewControl {
            request_id: vec![33; 16],
            ..renew.clone()
        };
        client
            .send(p::Kind::RenewControl, &newer_renew)
            .await
            .unwrap();
        let extended = grant(&mut client).await;
        assert!(extended.expires_at_ms > lost_renew.expires_at_ms);
        client.send(p::Kind::RenewControl, &renew).await.unwrap();
        assert_eq!(grant(&mut client).await, lost_renew);
        client.acquire_control([31; 16]).await.unwrap();
        let current = grant(&mut client).await;
        assert_eq!(current.lease_id, extended.lease_id);
        assert_eq!(current.generation, extended.generation);
        assert_eq!(current.expires_at_ms, extended.expires_at_ms);

        let release = p::ReleaseControl {
            request_id: vec![34; 16],
            lease_id: current.lease_id.clone(),
            generation: current.generation,
        };
        // Release has no successful reply in this binding. Both identical sends
        // precede a fresh acquisition on the same ordered connection.
        client
            .send(p::Kind::ReleaseControl, &release)
            .await
            .unwrap();
        client
            .send(p::Kind::ReleaseControl, &release)
            .await
            .unwrap();
        client.acquire_control([35; 16]).await.unwrap();
        let fresh = grant(&mut client).await;
        assert!(fresh.generation > current.generation);
        assert_ne!(fresh.lease_id, current.lease_id);
        client
            .send(p::Kind::ReleaseControl, &release)
            .await
            .unwrap();
        client
            .send(
                p::Kind::ReleaseControl,
                &p::ReleaseControl {
                    request_id: vec![36; 16],
                    ..release
                },
            )
            .await
            .unwrap();
        client.acquire_control([35; 16]).await.unwrap();
        assert_eq!(grant(&mut client).await, fresh);

        let old_command = activate(&client, 3, &current, 1, 1).encode_to_vec();
        client.submit(&old_command).await.unwrap();
        match client.receive().await.unwrap() {
            ClientEvent::CommandResult(result) => {
                assert_eq!(result.status, "Rejected");
                assert_eq!(result.rejection, "StaleControl");
                assert!(result.source_commit.is_empty());
            }
            event => panic!("unexpected old-generation result: {event:?}"),
        }
        assert_eq!(server.host.lock().await.diagnostics().counter, 0);
        let fresh_command = activate(&client, 3, &fresh, 2, 1).encode_to_vec();
        client.submit(&fresh_command).await.unwrap();
        let result = result_and_revision(&mut client, 2).await;
        assert_eq!(result.status, "Applied");
        assert_eq!(server.host.lock().await.diagnostics().counter, 1);
        server.shutdown().await;
    })
    .await
    .expect("live TLS lease retry scenario deadline");
}
