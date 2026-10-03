#[path = "common/resources.rs"]
#[allow(dead_code)]
mod fixture;
use fixture::*;
use prost::Message;
use rui_protocol_core::{
    resources::ResourceContext,
    transcript::{Direction, RecordingPolicy, TranscriptLimits},
};
use rui_transport::{
    diagnostics::Diagnostics, proto as p, ClientEvent, ServerConfig, TransportError,
};

fn scope(owner: &Diagnostics, view: [u8; 16]) -> ResourceContext {
    owner
        .scopes()
        .unwrap()
        .into_iter()
        .find(|s| s.view_id == view)
        .unwrap()
}
async fn pong(client: &mut rui_transport::NetworkClient, nonce: u64) {
    client
        .send(p::Kind::Ping, &p::Ping { nonce })
        .await
        .unwrap();
    assert!(matches!(client.receive().await.unwrap(), ClientEvent::Pong(n) if n == nonce));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn exact_post_auth_trace_binds_resources_commands_replies_and_revocation() {
    let diagnostics = Diagnostics::new(
        RecordingPolicy::AuthorizedDiagnosticBytes,
        TranscriptLimits::default(),
        2,
    )
    .unwrap();
    let (server, credential, observer_credential, pairing) = setup_with_config(ServerConfig {
        diagnostics: Some(diagnostics.clone()),
        max_message_bytes: 1024,
        ..Default::default()
    })
    .await;
    let mut controller = connect(&server, &credential, 3).await;
    let mut observer = connect(&server, &observer_credential, 4).await;
    controller.open_view().await.unwrap();
    observer.open_view().await.unwrap();
    tokio::join!(applied(&mut controller, 1), applied(&mut observer, 1));
    controller.presented(1).await.unwrap();
    controller.acquire_control([9; 16]).await.unwrap();
    let ClientEvent::ControlGranted(grant) = controller.receive().await.unwrap() else {
        panic!()
    };
    let command = activate(&controller, &grant, 1);
    controller.submit(&command).await.unwrap();
    tokio::join!(applied(&mut controller, 2), applied(&mut observer, 2));
    pong(&mut controller, 17).await;
    pong(&mut observer, 18).await;
    let context = scope(&diagnostics, controller.view_id);
    let observed = scope(&diagnostics, observer.view_id);
    for context in [context, observed] {
        diagnostics
            .inspect(context, |entries| {
                let mut kinds = std::collections::BTreeSet::new();
                for entry in entries {
                    let exact = entry.exact_bytes().unwrap();
                    assert!(!exact.windows(SECRET.len()).any(|v| v == SECRET.as_bytes()));
                    assert!(!exact
                        .windows(credential.expose_for_tls_authentication().len())
                        .any(|v| v == credential.expose_for_tls_authentication()));
                    let packet = p::Packet::decode(exact).unwrap();
                    assert_eq!(packet.kind as u32, entry.kind);
                    assert_eq!(packet.session_id, context.scope.session_id);
                    assert_eq!(packet.epoch, context.scope.epoch);
                    assert_eq!(packet.view_id, context.view_id);
                    assert!(!matches!(
                        p::Kind::try_from(packet.kind).unwrap(),
                        p::Kind::ClientHello | p::Kind::ServerHello
                    ));
                    if packet.kind == p::Kind::SemanticCommand as i32 {
                        assert_eq!(entry.direction, Direction::ClientToHost);
                        assert_eq!(packet.payload, command);
                    }
                    kinds.insert(packet.kind);
                }
                for kind in [
                    p::Kind::OpenView,
                    p::Kind::ViewOpened,
                    p::Kind::SnapshotBegin,
                    p::Kind::PublicationChunk,
                    p::Kind::PublicationCommit,
                    p::Kind::TransactionBegin,
                    p::Kind::ResourceOffer,
                    p::Kind::ResourceChunk,
                    p::Kind::ResourceAvailable,
                    p::Kind::AppliedAck,
                    p::Kind::Ping,
                    p::Kind::Pong,
                ] {
                    assert!(kinds.contains(&(kind as i32)), "missing {kind:?}");
                }
            })
            .unwrap();
    }
    let mut wrong = context;
    wrong.scope.principal_id = [0; 16];
    assert_eq!(
        diagnostics.inspect(wrong, |_| ()).unwrap_err(),
        TransportError::Authority
    );
    pairing.lock().unwrap().revoke_local([3; 16]);
    assert_eq!(
        diagnostics.inspect(context, |_| ()).unwrap_err(),
        TransportError::Authority
    );
    assert!(diagnostics.inspect(observed, |_| ()).is_ok());
    observer.close().await.unwrap();
    server.shutdown().await;
    assert!(diagnostics.scopes().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn metadata_diagnostics_never_retains_bodies_and_capacity_is_explicit() {
    let diagnostics = Diagnostics::new(
        RecordingPolicy::MetadataOnly,
        TranscriptLimits::default(),
        1,
    )
    .unwrap();
    let (server, credential, observer_credential, _) = setup_with_config(ServerConfig {
        diagnostics: Some(diagnostics.clone()),
        max_message_bytes: 1024,
        ..Default::default()
    })
    .await;
    let mut controller = connect(&server, &credential, 3).await;
    controller.open_view().await.unwrap();
    applied(&mut controller, 1).await;
    pong(&mut controller, 1).await;
    let context = scope(&diagnostics, controller.view_id);
    diagnostics
        .inspect(context, |entries| {
            let mut count = 0;
            for entry in entries {
                assert!(entry.exact_bytes().is_none());
                count += 1;
            }
            assert!(count > 10);
        })
        .unwrap();
    let result = rui_transport::NetworkClient::connect(
        server.address,
        "localhost".into(),
        include_bytes!("data/localhost.der").to_vec(),
        &observer_credential,
        hello(4),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(diagnostics.scopes().unwrap(), vec![context]);
    pong(&mut controller, 2).await;
    controller.close().await.unwrap();
    server.shutdown().await;
    assert!(diagnostics.scopes().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn recorder_exhaustion_closes_connection_without_leaking_scope_or_reservation() {
    let diagnostics = Diagnostics::new(
        RecordingPolicy::MetadataOnly,
        TranscriptLimits {
            records: 1,
            message_bytes: 1024,
            retained_bytes: 4096,
        },
        1,
    )
    .unwrap();
    let (server, credential, _, _) = setup_with_config(ServerConfig {
        diagnostics: Some(diagnostics.clone()),
        max_message_bytes: 1024,
        ..Default::default()
    })
    .await;
    let mut client = connect(&server, &credential, 3).await;
    let context = scope(&diagnostics, client.view_id);
    let charged = server
        .host
        .lock()
        .await
        .retained_memory_bytes(&authenticated(3), client.view_id)
        .unwrap();
    assert!(charged >= 8192); // Complete reserved diagnostic arena, before recording.
    client.open_view().await.unwrap();
    // OpenView occupies the one slot; ViewOpened cannot silently go unrecorded.
    assert!(client.receive().await.is_err());
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !diagnostics.scopes().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        diagnostics.inspect(context, |_| ()).unwrap_err(),
        TransportError::Authority
    );
    server.shutdown().await;
}
