//! Real TLS resource lifecycle with a bounded synthetic source shaper. Actual
//! native font/library rendering is a separate explicit pinned-environment gate.
use prost::Message;
use rui_host_broker::*;
use rui_protocol_core::{
    commands::AuthenticatedClient,
    model::{Record, TextStatus},
    resources::*,
    session::*,
    wire::proto as core,
};
use rui_transport::{
    pairing::{Credential, PairingAuthority},
    proto as p, ClientEvent, HostIdentity, NetworkClient, ServerConfig, ServerHandle,
};
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};
const SECRET: &str = "RESOURCE-TRANSPORT-PRIVATE-SENTINEL";
struct SyntheticPublicShaper;
impl SourceFontProvider for SyntheticPublicShaper {
    fn shape(&self, text: &str, size: f64) -> Result<SourceShapedText, SourceFontError> {
        assert!(!text.contains(SECRET));
        let mut resources = BTreeMap::new();
        let mut glyphs = Vec::new();
        for (i, c) in text.chars().enumerate() {
            let mut body = b"RVP1".to_vec();
            body.extend_from_slice(&42u32.to_be_bytes());
            body.push(1);
            body.extend_from_slice(&0.0f64.to_be_bytes());
            body.extend_from_slice(&0.0f64.to_be_bytes());
            for j in 0..40 {
                body.push(2);
                body.extend_from_slice(&((c as u32) as f64 / 65536.0).to_be_bytes());
                body.extend_from_slice(&(j as f64 / 64.0).to_be_bytes());
            }
            body.push(5);
            let digest = resource_digest(
                ResourceKind::GlyphOutline,
                1,
                ResourceProvenance::FontOutline,
                &body,
            );
            resources.entry(digest).or_insert(SourceOutline::new(
                ResourceOffer {
                    kind: ResourceKind::GlyphOutline,
                    version: 1,
                    provenance: ResourceProvenance::FontOutline,
                    total_bytes: body.len() as u64,
                    digest,
                },
                body,
            )?);
            glyphs.push(SourceGlyph {
                resource: digest,
                size,
                position: [i as f64 * 7.0, 12.0],
            });
        }
        Ok(SourceShapedText {
            glyphs,
            resources: resources.into_values().collect(),
        })
    }
}
fn auth(id: u8, control: bool) -> VerifiedAuthorization {
    VerifiedAuthorization {
        principal: format!("resource-client-{id}"),
        policy_version: 1,
        permissions: Permissions {
            can_view: true,
            can_control: control,
            actions: if control {
                ["Activate".into()].into_iter().collect()
            } else {
                Default::default()
            },
        },
    }
}
fn authenticated(id: u8) -> AuthenticatedClient {
    AuthenticatedClient {
        principal: format!("resource-client-{id}"),
        client_instance_id: [id; 16],
    }
}
fn hello(id: u8) -> ClientHello {
    let mut h = ClientHello {
        client_instance_id: [id; 16],
        supported_versions: [1].into_iter().collect(),
        supported_profiles: SessionConfig::default().supported_profiles,
        required_profiles: SessionConfig::default().required_profiles,
        receiver_limits: ReceiverLimits {
            envelope_bytes: 1024,
            chunk_bytes: 512,
            ..Default::default()
        },
        resume: None,
    };
    h.supported_profiles
        .insert("glyph.outline-single.v1".into());
    h
}
async fn setup() -> (
    ServerHandle,
    Credential,
    Credential,
    Arc<Mutex<PairingAuthority>>,
) {
    let mut pairing = PairingAuthority::new(2).unwrap();
    let mut credentials = Vec::new();
    for id in [3, 4] {
        let request = pairing.request_local_pairing([id; 16]).unwrap();
        credentials.push(
            pairing
                .approve_local(request.request_id, auth(id, id == 3))
                .unwrap(),
        );
    }
    let host = ControlledHost::new_corpus_with_fonts(
        HostConfig {
            max_text_bytes: 256,
            session: SessionConfig {
                max_history_records: 1,
                ..Default::default()
            },
            ..Default::default()
        },
        SECRET.into(),
        Arc::new(SyntheticPublicShaper),
    )
    .unwrap();
    let identity = HostIdentity::from_der(
        include_bytes!("data/localhost.der").to_vec(),
        include_bytes!("data/localhost-key.der").to_vec(),
    )
    .unwrap();
    let pairing = Arc::new(Mutex::new(pairing));
    let server = ServerHandle::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        identity,
        host,
        pairing.clone(),
        ServerConfig {
            max_message_bytes: 1024,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    (
        server,
        credentials.remove(0),
        credentials.remove(0),
        pairing,
    )
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
async fn applied(client: &mut NetworkClient, revision: u64) -> (usize, usize) {
    let mut need = 0;
    let mut available = 0;
    for _ in 0..1024 {
        match client.receive().await.unwrap() {
            ClientEvent::ResourcesNeeded { revision: r } => {
                assert_eq!(r, revision);
                need += 1;
            }
            ClientEvent::ResourceAvailable => available += 1,
            ClientEvent::Applied { revision: r } => {
                assert_eq!(r, revision);
                client.flush_pending().await.unwrap();
                return (need, available);
            }
            ClientEvent::CommandResult(r) => assert_eq!(r.status, "Applied"),
            ClientEvent::ViewOpened(view) => {
                assert_eq!(view.coverage, "complete");
                assert_eq!(view.canvas_width, 920.0);
            }
            other => panic!("unexpected public event {other:?}"),
        }
    }
    panic!("bounded publication did not apply")
}
fn activate(client: &NetworkClient, grant: &p::ControlGranted, sequence: u64) -> Vec<u8> {
    let state = client.receiver().state().unwrap();
    let Record::Semantic(node) = &state.records[&COUNTER_ID] else {
        panic!()
    };
    core::SemanticCommand {
        epoch: client.epoch.to_vec(),
        view_id: client.view_id.to_vec(),
        client_instance_id: vec![3; 16],
        command_seq: sequence,
        control_lease_id: grant.lease_id.clone(),
        control_generation: grant.generation,
        target_id: COUNTER_ID,
        action_id: "Activate".into(),
        action_schema_version: 1,
        observed_view_revision: state.revision,
        preconditions: Some(core::Preconditions {
            target_version: Some(node.version),
            ..Default::default()
        }),
        arguments: Some(core::semantic_command::Arguments::NoArguments(
            core::EmptyArguments {},
        )),
    }
    .encode_to_vec()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn real_tls_paged_cold_closure_warm_updates_and_live_cache_loss_are_coherent() {
    let (server, controller_credential, observer_credential, _) = setup().await;
    let mut controller = connect(&server, &controller_credential, 3).await;
    let mut observer = connect(&server, &observer_credential, 4).await;
    controller.open_view().await.unwrap();
    observer.open_view().await.unwrap();
    let (a, b) = tokio::join!(applied(&mut controller, 1), applied(&mut observer, 1));
    assert_eq!(a.0, 1);
    assert_eq!(b.0, 1);
    assert!(a.1 > 19);
    assert_eq!(a, b); // request spans >one1024-byte page
    let cold_bytes = observer.receiver().cache_bytes().unwrap();
    assert!(cold_bytes > 0);
    {
        let host = server.host.lock().await;
        let capture = host
            .source_capture(&authenticated(4), observer.view_id)
            .unwrap();
        assert_eq!(
            capture.state().records,
            observer.receiver().state().unwrap().records
        );
        assert_eq!(capture.resources().count(), b.1 + 1); // last completion returns Applied
        assert!(
            host.retained_memory_bytes(&authenticated(4), observer.view_id)
                .unwrap()
                <= observer.limits.view_memory_bytes
        );
    }
    controller.presented(1).await.unwrap();
    controller.acquire_control([9; 16]).await.unwrap();
    let ClientEvent::ControlGranted(grant) = controller.receive().await.unwrap() else {
        panic!()
    };
    let command = activate(&controller, &grant, 1);
    controller.submit(&command).await.unwrap();
    let (a, b) = tokio::join!(applied(&mut controller, 2), applied(&mut observer, 2));
    assert_eq!(a, (0, 0));
    assert_eq!(b, (0, 0));
    assert_eq!(observer.receiver().cache_bytes().unwrap(), cold_bytes);
    controller.submit(&command).await.unwrap();
    assert!(
        matches!(controller.receive().await.unwrap(),ClientEvent::CommandResult(r) if r.status=="Applied")
    );
    assert_eq!(server.host.lock().await.diagnostics().counter, 1);
    observer.resource_cache_lost().await.unwrap();
    assert_eq!(observer.receiver().presented_revision().unwrap(), None);
    assert!(!observer.receiver().resources_ready().unwrap());
    assert_eq!(observer.receiver().state().unwrap().revision, 2);
    let recovery = applied(&mut observer, 2).await;
    assert!(recovery.1 > 19);
    assert!(observer.receiver().resources_ready().unwrap());
    let live_bytes: usize = server
        .host
        .lock()
        .await
        .source_capture(&authenticated(4), observer.view_id)
        .unwrap()
        .resources()
        .map(|(_, body)| body.len())
        .sum();
    // Warm cache may also retain retired outlines. Loss restores precisely the
    // current graph closure, rather than refetching historical unpinned entries.
    assert_eq!(observer.receiver().cache_bytes().unwrap(), live_bytes);
    assert!(live_bytes <= cold_bytes);
    let Record::Text(private) = &observer.receiver().state().unwrap().records[&SECRET_TEXT_ID]
    else {
        panic!()
    };
    assert_eq!(private.status, TextStatus::Redacted);
    assert!(private.content.is_none());
    assert!(!format!("{:?}", observer.receiver().state().unwrap()).contains(SECRET));
    assert!(observer.receiver().memory_used() <= observer.limits.view_memory_bytes);
    assert_eq!(server.host.lock().await.diagnostics().consumed_commands, 1);
    controller.close().await.unwrap();
    observer.close().await.unwrap();
    server.shutdown().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn premature_applied_cannot_bypass_pending_resource_verification() {
    let (server, credential, _, _) = setup().await;
    let mut client = connect(&server, &credential, 3).await;
    client.open_view().await.unwrap();
    assert!(matches!(
        client.receive().await.unwrap(),
        ClientEvent::ViewOpened(_)
    ));
    assert!(matches!(
        client.receive().await.unwrap(),
        ClientEvent::ResourcesNeeded { revision: 1 }
    ));
    client
        .send(p::Kind::AppliedAck, &p::RevisionAck { revision: 1 })
        .await
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(3), client.receive())
        .await
        .unwrap();
    assert!(event.is_err());
    assert_eq!(client.receiver().state().unwrap().revision, 0);
    assert_eq!(server.host.lock().await.diagnostics().consumed_commands, 0);
    server.shutdown().await;
}
fn known_digest(server: &ControlledHost, client: &NetworkClient) -> Vec<u8> {
    server
        .source_capture(&authenticated(3), client.view_id)
        .unwrap()
        .resources()
        .next()
        .unwrap()
        .0
        .digest
        .to_vec()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_eviction_ids_cannot_mutate_readiness_or_disclose_resources() {
    for case in 0..3 {
        let (server, credential, _, _) = setup().await;
        let mut client = connect(&server, &credential, 3).await;
        client.open_view().await.unwrap();
        applied(&mut client, 1).await;
        let digest = known_digest(&*server.host.lock().await, &client);
        let digests = match case {
            0 => vec![vec![8; 31]],
            1 => vec![vec![8; 32]],
            _ => vec![digest.clone(), digest],
        };
        client
            .send(p::Kind::ResourceEvicted, &p::ResourceIds { digests })
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(3), client.receive())
                .await
                .unwrap()
                .is_err()
        );
        assert_eq!(server.host.lock().await.diagnostics().counter, 0);
        server.shutdown().await;
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declared_eviction_blocks_reapplied_even_before_any_need_request() {
    let (server, credential, _, _) = setup().await;
    let mut client = connect(&server, &credential, 3).await;
    client.open_view().await.unwrap();
    applied(&mut client, 1).await;
    let digest = known_digest(&*server.host.lock().await, &client);
    client
        .send(
            p::Kind::ResourceEvicted,
            &p::ResourceIds {
                digests: vec![digest],
            },
        )
        .await
        .unwrap();
    client
        .send(p::Kind::AppliedAck, &p::RevisionAck { revision: 1 })
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), client.receive())
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(server.host.lock().await.diagnostics().counter, 0);
    server.shutdown().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_approval_revocation_prevents_queued_resource_request_delivery() {
    let (server, credential, _, pairing) = setup().await;
    let mut client = connect(&server, &credential, 3).await;
    client.open_view().await.unwrap();
    assert!(matches!(
        client.receive().await.unwrap(),
        ClientEvent::ViewOpened(_)
    ));
    assert!(matches!(
        client.receive().await.unwrap(),
        ClientEvent::ResourcesNeeded { revision: 1 }
    ));
    // receive() returns before flushing the queued NeedResources. Local approval
    // is revoked before that request reaches the authenticated server handler.
    assert!(pairing.lock().unwrap().revoke_local([3; 16]));
    assert!(
        tokio::time::timeout(Duration::from_secs(3), client.receive())
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(client.receiver().cache_bytes().unwrap(), 0);
    assert_eq!(client.receiver().state().unwrap().revision, 0);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acknowledgement_of_retained_inflight_transaction_survives_history_eviction() {
    let (server, credential, _, _) = setup().await;
    let mut client = connect(&server, &credential, 3).await;
    client.open_view().await.unwrap();
    applied(&mut client, 1).await;
    client.presented(1).await.unwrap();
    client.acquire_control([9; 16]).await.unwrap();
    let ClientEvent::ControlGranted(grant) = client.receive().await.unwrap() else {
        panic!()
    };
    client.submit(&activate(&client, &grant, 1)).await.unwrap();
    for _ in 0..16 {
        match client.receive().await.unwrap() {
            ClientEvent::CommandResult(result) => assert_eq!(result.status, "Applied"),
            ClientEvent::Applied { revision: 2 } => break,
            other => panic!("unexpected public event {other:?}"),
        }
    }
    assert_eq!(client.receiver().state().unwrap().revision, 2);
    // The AppliedAck remains queued locally. A trusted source invocation now
    // publishes revision3; one-entry host history evicts revision2 before ACK.
    let command = activate(&client, &grant, 2);
    let result = server
        .host
        .lock()
        .await
        .submit(&authenticated(3), &command, 1)
        .unwrap();
    assert_eq!(
        result.status,
        rui_protocol_core::commands::CommandStatus::Applied
    );
    client.flush_pending().await.unwrap();
    assert_eq!(applied(&mut client, 3).await, (0, 0));
    assert_eq!(server.host.lock().await.diagnostics().counter, 2);
    assert_eq!(server.host.lock().await.diagnostics().consumed_commands, 2);
    client.close().await.unwrap();
    server.shutdown().await;
}
