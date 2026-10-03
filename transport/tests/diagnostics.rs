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

// Inspect complete reconstructed bodies as well as envelopes: a sentinel can
// cross an arbitrary publication/resource chunk boundary.
fn assert_no_private_bytes(bytes: &[u8]) {
    for secret in [SECRET, EDITED_SECRET] {
        assert!(!bytes
            .windows(secret.len())
            .any(|part| part == secret.as_bytes()));
    }
}
fn inspect_private_trace(owner: &Diagnostics, view: [u8; 16], exact: bool, observer: bool) {
    owner
        .inspect(scope(owner, view), |entries| {
            let mut publications = std::collections::BTreeMap::<Vec<u8>, Vec<u8>>::new();
            let mut resources = std::collections::BTreeMap::<Vec<u8>, Vec<u8>>::new();
            let mut saw_private_command = false;
            for entry in entries {
                assert_no_private_bytes(format!("{entry:?}").as_bytes());
                if !exact {
                    assert!(entry.exact_bytes().is_none());
                    continue;
                }
                let bytes = entry.exact_bytes().unwrap();
                let packet = p::Packet::decode(bytes).unwrap();
                if observer {
                    assert_no_private_bytes(bytes);
                } else if packet.kind == p::Kind::SemanticCommand as i32 {
                    saw_private_command |= packet
                        .payload
                        .windows(EDITED_SECRET.len())
                        .any(|part| part == EDITED_SECRET.as_bytes());
                }
                if packet.kind == p::Kind::PublicationChunk as i32 {
                    let chunk = p::PublicationChunk::decode(packet.payload.as_slice()).unwrap();
                    publications
                        .entry(chunk.publication_id)
                        .or_default()
                        .extend(chunk.data);
                }
                if packet.kind == p::Kind::ResourceChunk as i32 {
                    let chunk = p::ResourceChunk::decode(packet.payload.as_slice()).unwrap();
                    resources
                        .entry(chunk.digest)
                        .or_default()
                        .extend(chunk.data);
                }
            }
            if exact && observer {
                assert!(!publications.is_empty());
                for body in publications.values().chain(resources.values()) {
                    assert_no_private_bytes(body);
                }
            }
            if exact && !observer {
                // Explicit trusted byte diagnostics may record the controller's
                // own private command; metadata/debug output must never contain it.
                assert!(saw_private_command);
            }
        })
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn private_edit_stays_out_of_observer_resources_history_and_ordinary_diagnostics() {
    use rui_host_broker::{READ_SECRET_PERMISSION, SECRET_ID, SECRET_TEXT_ID};
    use rui_protocol_core::{
        model::{Record, TextStatus},
        wire::proto as core,
    };
    for policy in [
        RecordingPolicy::MetadataOnly,
        RecordingPolicy::AuthorizedDiagnosticBytes,
    ] {
        let diagnostics = Diagnostics::new(
            policy,
            TranscriptLimits {
                retained_bytes: 2 * 1024 * 1024,
                ..Default::default()
            },
            2,
        )
        .unwrap();
        let mut authorization = auth(3, true);
        authorization.permissions.actions.extend([
            "Focus".into(),
            "ReplaceText".into(),
            READ_SECRET_PERMISSION.into(),
        ]);
        let (server, credential, observer_credential, _) = setup_with_authorization(
            ServerConfig {
                diagnostics: Some(diagnostics.clone()),
                max_message_bytes: 1024,
                ..Default::default()
            },
            authorization,
        )
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
        let mut focus =
            core::SemanticCommand::decode(activate(&controller, &grant, 1).as_slice()).unwrap();
        focus.target_id = SECRET_ID;
        focus.action_id = "Focus".into();
        focus.preconditions.as_mut().unwrap().target_version = Some(1);
        controller.submit(&focus.encode_to_vec()).await.unwrap();
        tokio::join!(applied(&mut controller, 2), applied(&mut observer, 2));
        pong(&mut observer, 41).await;
        inspect_private_trace(
            &diagnostics,
            observer.view_id,
            policy == RecordingPolicy::AuthorizedDiagnosticBytes,
            true,
        );
        let view = observer.view_id;
        let mut retained = observer.into_retained();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while server
                .host
                .lock()
                .await
                .view_status(&authenticated(4), view)
                .unwrap()
                .connected
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let state = controller.receiver().state().unwrap();
        let Record::Semantic(target) = &state.records[&SECRET_ID] else {
            panic!()
        };
        let Record::Text(text) = &state.records[&SECRET_TEXT_ID] else {
            panic!()
        };
        let focus_generation = state
            .records
            .values()
            .find_map(|record| match record {
                Record::Session(session) => Some(session.focus_generation),
                _ => None,
            })
            .unwrap();
        let mut edit = focus;
        edit.command_seq = 2;
        edit.action_id = "ReplaceText".into();
        edit.observed_view_revision = state.revision;
        edit.preconditions = Some(core::Preconditions {
            target_version: Some(target.version),
            text_version: Some(text.version),
            focus_generation: Some(focus_generation),
            ..Default::default()
        });
        edit.arguments = Some(core::semantic_command::Arguments::ReplaceText(
            core::TextArguments {
                start: 0,
                end: SECRET.chars().count() as u64,
                text: EDITED_SECRET.into(),
            },
        ));
        controller.submit(&edit.encode_to_vec()).await.unwrap();
        applied(&mut controller, 3).await;
        pong(&mut controller, 42).await;
        assert!(
            matches!(&controller.receiver().state().unwrap().records[&SECRET_TEXT_ID], Record::Text(t) if t.content.as_deref() == Some(EDITED_SECRET))
        );
        inspect_private_trace(
            &diagnostics,
            controller.view_id,
            policy == RecordingPolicy::AuthorizedDiagnosticBytes,
            false,
        );
        let mut observer = retained
            .reconnect(
                server.address,
                "localhost".into(),
                include_bytes!("data/localhost.der").to_vec(),
                &observer_credential,
                hello(4),
            )
            .await
            .unwrap();
        assert_eq!(observer.view_id, view);
        observer.open_view().await.unwrap();
        assert!(matches!(
            observer.receive().await.unwrap(),
            ClientEvent::ViewOpened(_)
        ));
        assert!(matches!(
            observer.receive().await.unwrap(),
            ClientEvent::ResumeAccepted(_)
        ));
        applied(&mut observer, 3).await;
        pong(&mut observer, 43).await;
        let state = observer.receiver().state().unwrap();
        assert!(
            matches!(&state.records[&SECRET_TEXT_ID], Record::Text(t) if t.status == TextStatus::Redacted && t.content.is_none())
        );
        assert_no_private_bytes(format!("{state:?}").as_bytes());
        assert!(observer.receiver().cache_bytes().unwrap() > 0);
        for resource in state.resource_refs() {
            assert_no_private_bytes(observer.receiver().read_resource(resource).unwrap());
        }
        let host = server.host.lock().await;
        assert_no_private_bytes(&host.snapshot_publication(&authenticated(4), view).unwrap());
        assert_no_private_bytes(format!("{:?}", host.diagnostics()).as_bytes());
        let source = host.source_capture(&authenticated(4), view).unwrap();
        for (_, body) in source.resources() {
            assert_no_private_bytes(body);
        }
        drop(source);
        drop(host);
        inspect_private_trace(
            &diagnostics,
            view,
            policy == RecordingPolicy::AuthorizedDiagnosticBytes,
            true,
        );
        observer.close().await.unwrap();
        controller.close().await.unwrap();
        server.shutdown().await;
        assert!(diagnostics.scopes().unwrap().is_empty());
    }
}
