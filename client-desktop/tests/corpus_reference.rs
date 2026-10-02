//! Provisional direct-source comparison for the explicit Partial glyph corpus.
//! These two receivers never report Presented; selected vector-root comparison
//! is not whole-view completeness or the full native desktop M2 gate.
#![cfg(feature = "desktop")]
use prost::Message;
use rui_client_desktop::{
    interaction::{command, CommandBinding},
    RenderError, RenderLimits, Renderer, Viewport,
};
use rui_host_broker::{
    ControlledHost, HostConfig, CHECKBOX_ID, PUBLIC_TEXT_ID, PUBLIC_TEXT_MODEL_ID,
};
use rui_protocol_core::{
    commands::{Arguments, AuthenticatedClient},
    model::{ObjectKind, Record, TextRange, TypedValue, VisualCoverage},
    session::{ClientHello, Permissions, ReceiverLimits, SessionConfig, VerifiedAuthorization},
    wire::proto,
};
use rui_transport::{
    pairing::{Credential, PairingAuthority},
    ClientEvent, HostIdentity, NetworkClient, ServerConfig, ServerHandle,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

async fn receive(client: &mut NetworkClient) -> ClientEvent {
    tokio::time::timeout(Duration::from_secs(3), client.receive())
        .await
        .unwrap()
        .unwrap()
}
async fn connect(
    server: &ServerHandle,
    credential: &Credential,
    id: u8,
    semantic: bool,
) -> NetworkClient {
    let mut profiles = SessionConfig::default().supported_profiles;
    let mut required = SessionConfig::default().required_profiles;
    if semantic {
        profiles.insert("semantics.only.v1".into());
        required.insert("semantics.only.v1".into());
    }
    let mut client = NetworkClient::connect(
        server.address,
        "localhost".into(),
        include_bytes!("../../transport/tests/data/localhost.der").to_vec(),
        credential,
        ClientHello {
            client_instance_id: [id; 16],
            supported_versions: [1].into_iter().collect(),
            supported_profiles: profiles,
            required_profiles: required,
            receiver_limits: ReceiverLimits::default(),
            resume: None,
        },
    )
    .await
    .unwrap();
    client.open_view().await.unwrap();
    assert!(matches!(
        receive(&mut client).await,
        ClientEvent::ViewOpened(_)
    ));
    assert!(matches!(
        receive(&mut client).await,
        ClientEvent::Applied { revision: 1 }
    ));
    client
}
fn encode(c: rui_protocol_core::commands::SemanticCommand) -> Vec<u8> {
    let arguments = match c.arguments {
        Arguments::Checked(checked) => {
            proto::semantic_command::Arguments::SetValue(proto::ValueArguments {
                value: Some(proto::value_arguments::Value::Checked(checked)),
            })
        }
        Arguments::ReplaceText { start, end, text } => {
            proto::semantic_command::Arguments::ReplaceText(proto::TextArguments {
                start,
                end,
                text,
            })
        }
        _ => panic!("unexpected test action"),
    };
    proto::SemanticCommand {
        epoch: c.epoch.to_vec(),
        view_id: c.view_id.to_vec(),
        client_instance_id: c.client_instance_id.to_vec(),
        command_seq: c.command_seq,
        control_lease_id: c.control_lease_id.to_vec(),
        control_generation: c.control_generation,
        target_id: c.target_id,
        action_id: c.action_id,
        action_schema_version: c.action_schema_version,
        observed_view_revision: c.observed_view_revision,
        preconditions: Some(proto::Preconditions {
            target_version: c.preconditions.target_version,
            text_version: c.preconditions.text_version,
            model_version: c.preconditions.model_version,
            focus_generation: c.preconditions.focus_generation,
        }),
        arguments: Some(arguments),
    }
    .encode_to_vec()
}
async fn compare(server: &ServerHandle, client: &NetworkClient, id: u8) -> u64 {
    let source = server
        .host
        .lock()
        .await
        .source_capture(
            &AuthenticatedClient {
                principal: format!("corpus-{id}"),
                client_instance_id: [id; 16],
            },
            client.view_id,
        )
        .unwrap();
    let state = client.receiver().state().unwrap();
    assert_eq!(source.state().revision, state.revision);
    assert_eq!(source.state().records, state.records);
    let renderer =
        Renderer::new(RenderLimits::default(), client.receiver().memory_budget()).unwrap();
    let viewport = Viewport {
        width: 230,
        height: 125,
        scale: 0.25,
        origin: rui_protocol_core::model::Point { x: 0.0, y: 0.0 },
    };
    assert!(matches!(
        renderer.render_view(source.state(), viewport, &()),
        Err(RenderError::IncompleteSurface(_))
    ));
    for surface in state.records.values().filter_map(|r| match r {
        Record::Surface(s) if s.mapped => Some(s),
        _ => None,
    }) {
        if surface.visual_coverage == VisualCoverage::Partial {
            assert!(surface
                .unsupported_features
                .contains(&"source-glyph-provider-pending".into()));
        }
        let root = surface.scene_root.unwrap();
        assert_eq!(root.kind, ObjectKind::Scene);
        let reference = renderer
            .render_scene(source.state(), root, viewport, &())
            .unwrap();
        let replay = renderer
            .render_scene(state, root, viewport, client.receiver())
            .unwrap();
        assert_eq!(reference.pixels(), replay.pixels());
    }
    assert_eq!(client.receiver().presented_revision().unwrap(), None);
    source.source_commit()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_corpus_direct_source_two_replicas_and_observer_viewports() {
    let mut pairing = PairingAuthority::new(4).unwrap();
    let mut credentials = Vec::new();
    for id in [11u8, 12, 13] {
        let request = pairing.request_local_pairing([id; 16]).unwrap();
        credentials.push(
            pairing
                .approve_local(
                    request.request_id,
                    VerifiedAuthorization {
                        principal: format!("corpus-{id}"),
                        policy_version: 1,
                        permissions: Permissions {
                            can_view: true,
                            can_control: id == 11,
                            actions: if id == 11 {
                                [
                                    "Activate",
                                    "SetValue",
                                    "Focus",
                                    "ReplaceText",
                                    "SetSelection",
                                    "ScrollTo",
                                ]
                                .into_iter()
                                .map(str::to_owned)
                                .collect()
                            } else {
                                Default::default()
                            },
                        },
                    },
                )
                .unwrap(),
        );
    }
    let server = ServerHandle::bind(
        "127.0.0.1:0".parse().unwrap(),
        HostIdentity::from_der(
            include_bytes!("../../transport/tests/data/localhost.der").to_vec(),
            include_bytes!("../../transport/tests/data/localhost-key.der").to_vec(),
        )
        .unwrap(),
        ControlledHost::new_corpus(HostConfig::default(), "private".into()).unwrap(),
        Arc::new(Mutex::new(pairing)),
        ServerConfig::default(),
    )
    .await
    .unwrap();
    let mut control = connect(&server, &credentials[0], 11, true).await;
    let mut a = connect(&server, &credentials[1], 12, false).await;
    let mut b = connect(&server, &credentials[2], 13, false).await;
    assert_eq!(compare(&server, &a, 12).await, 1);
    assert_eq!(compare(&server, &b, 13).await, 1);
    control.semantic_ready(1).await.unwrap();
    control.acquire_control([21; 16]).await.unwrap();
    let ClientEvent::ControlGranted(lease) = receive(&mut control).await else {
        panic!()
    };
    for (index, (target, action, args)) in [
        (CHECKBOX_ID, "SetValue", Arguments::Checked(true)),
        (
            PUBLIC_TEXT_ID,
            "ReplaceText",
            Arguments::ReplaceText {
                start: 0,
                end: 6,
                text: "A😀e\u{301}אב".into(),
            },
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let c = command(
            control.receiver().state().unwrap(),
            target,
            action,
            args,
            CommandBinding {
                epoch: control.epoch,
                view: control.view_id,
                client: [11; 16],
                lease: lease.lease_id.as_slice().try_into().unwrap(),
                generation: lease.generation,
                sequence: index as u64 + 1,
            },
        )
        .unwrap();
        control.submit(&encode(c)).await.unwrap();
        let mut result = None;
        let mut applied = false;
        for _ in 0..32 {
            match receive(&mut control).await {
                ClientEvent::CommandResult(r) => {
                    assert_eq!(r.status, "Applied");
                    result = Some(r);
                }
                ClientEvent::Applied { .. } => applied = true,
                _ => {}
            }
            if applied && result.is_some() {
                break;
            }
        }
        assert!(applied && result.is_some());
        assert!(matches!(receive(&mut a).await, ClientEvent::Applied { .. }));
        assert!(matches!(receive(&mut b).await, ClientEvent::Applied { .. }));
        assert_eq!(compare(&server, &a, 12).await, index as u64 + 2);
        assert_eq!(compare(&server, &b, 13).await, index as u64 + 2);
        control
            .semantic_ready(control.receiver().state().unwrap().revision)
            .await
            .unwrap();
    }
    assert!(
        matches!(&a.receiver().state().unwrap().records[&CHECKBOX_ID],Record::Semantic(n) if n.value==Some(TypedValue::Boolean(true)))
    );
    assert!(
        matches!(&b.receiver().state().unwrap().records[&PUBLIC_TEXT_MODEL_ID],Record::Text(t) if t.content.as_deref()==Some("A😀e\u{301}אב") && t.selection==Some(TextRange{anchor:6,focus:6}))
    );
    let before = server.host.lock().await.diagnostics();
    let state = a.receiver().state().unwrap();
    let renderer = Renderer::new(RenderLimits::default(), a.receiver().memory_budget()).unwrap();
    let root = state
        .records
        .values()
        .find_map(|r| match r {
            Record::Surface(s) => s.scene_root,
            _ => None,
        })
        .unwrap();
    for viewport in [
        Viewport::new(80, 60),
        Viewport {
            width: 100,
            height: 70,
            scale: 0.5,
            origin: rui_protocol_core::model::Point { x: 150.0, y: 40.0 },
        },
    ] {
        renderer
            .render_scene(state, root, viewport, a.receiver())
            .unwrap();
    }
    let after = server.host.lock().await.diagnostics();
    assert_eq!(before.source_commit, after.source_commit);
    assert_eq!(before.consumed_commands, after.consumed_commands);
    assert_eq!(compare(&server, &a, 12).await, 3);
    control.close().await.unwrap();
    a.close().await.unwrap();
    b.close().await.unwrap();
    server.shutdown().await;
}
