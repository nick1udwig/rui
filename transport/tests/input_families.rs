//! Real TLS common-family admission into a cooperating headless source. The
//! synthetic outline provider and explicit test presentation callback do not
//! claim native font rendering, native GUI presentation, or OS injection.
use prost::Message;
use rui_host_broker::*;
use rui_protocol_core::{
    commands::{AuthenticatedClient, CommandFamily},
    model::{Primitive, Record, TextStatus},
    resources::*,
    session::*,
    wire::proto as p,
};
use rui_transport::{
    pairing::{Credential, PairingAuthority},
    proto as sp, ClientEvent, HostIdentity, NetworkClient, ServerConfig, ServerHandle,
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicU32, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
const SECRET: &str = "FAMILY-NETWORK-PRIVATE-SENTINEL";
const PREEDIT: &str = "PRIVATE-FAMILY-NETWORK-PREEDIT";
struct SyntheticOutline;
impl SourceFontProvider for SyntheticOutline {
    fn shape(&self, text: &str, size: f64) -> Result<SourceShapedText, SourceFontError> {
        assert!(!text.contains(SECRET));
        let mut body = b"RVP1".to_vec();
        body.extend_from_slice(&4u32.to_be_bytes());
        for (tag, x, y) in [(1, 0.0f64, 0.0f64), (2, 0.5, 0.0), (2, 0.5, 0.8)] {
            body.push(tag);
            body.extend_from_slice(&x.to_be_bytes());
            body.extend_from_slice(&y.to_be_bytes());
        }
        body.push(5);
        let digest = resource_digest(
            ResourceKind::GlyphOutline,
            1,
            ResourceProvenance::FontOutline,
            &body,
        );
        Ok(SourceShapedText {
            glyphs: text
                .chars()
                .enumerate()
                .map(|(i, _)| SourceGlyph {
                    resource: digest,
                    size,
                    position: [i as f64 * 7.0, 12.0],
                })
                .collect(),
            resources: vec![SourceOutline::new(
                ResourceOffer {
                    kind: ResourceKind::GlyphOutline,
                    version: 1,
                    provenance: ResourceProvenance::FontOutline,
                    total_bytes: body.len() as u64,
                    digest,
                },
                body,
            )?],
        })
    }
}
fn auth(id: u8) -> VerifiedAuthorization {
    VerifiedAuthorization {
        principal: format!("family-{id}"),
        policy_version: 1,
        permissions: Permissions {
            can_view: true,
            can_control: id == 3,
            actions: if id == 3 {
                [
                    "Activate",
                    "Focus",
                    "ReplaceText",
                    "RawInputBatch",
                    "TextComposition",
                    "ResizeRequest",
                    READ_SECRET_PERMISSION,
                ]
                .into_iter()
                .map(str::to_owned)
                .collect()
            } else {
                Default::default()
            },
        },
    }
}
fn client(id: u8) -> AuthenticatedClient {
    AuthenticatedClient {
        principal: format!("family-{id}"),
        client_instance_id: [id; 16],
    }
}
fn hello(id: u8) -> ClientHello {
    let mut hello = ClientHello {
        client_instance_id: [id; 16],
        supported_versions: [1].into_iter().collect(),
        supported_profiles: SessionConfig::default().supported_profiles,
        required_profiles: SessionConfig::default().required_profiles,
        receiver_limits: ReceiverLimits {
            envelope_bytes: 2048,
            chunk_bytes: 512,
            ..Default::default()
        },
        resume: None,
    };
    hello
        .supported_profiles
        .insert("glyph.outline-single.v1".into());
    hello
}
#[derive(Default)]
struct CleanupProbe {
    count: AtomicUsize,
    buttons: AtomicU32,
}
async fn setup() -> (ServerHandle, Credential, Credential, Arc<CleanupProbe>) {
    let mut pairing = PairingAuthority::new(2).unwrap();
    let mut credentials = Vec::new();
    for id in [3, 4] {
        let request = pairing.request_local_pairing([id; 16]).unwrap();
        credentials.push(pairing.approve_local(request.request_id, auth(id)).unwrap());
    }
    let host = ControlledHost::new_corpus_with_fonts(
        HostConfig {
            max_text_bytes: 256,
            ..Default::default()
        },
        SECRET.into(),
        Arc::new(SyntheticOutline),
    )
    .unwrap();
    let cleanup = Arc::new(CleanupProbe::default());
    let probe = cleanup.clone();
    let server = ServerHandle::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        HostIdentity::from_der(
            include_bytes!("data/localhost.der").to_vec(),
            include_bytes!("data/localhost-key.der").to_vec(),
        )
        .unwrap(),
        host,
        Arc::new(Mutex::new(pairing)),
        ServerConfig {
            max_message_bytes: 2048,
            cleanup: Arc::new(move |notice| {
                assert!(notice.keys.is_empty());
                assert!(notice.buttons.iter().all(|b| *b == 1));
                probe.count.fetch_add(1, Ordering::Relaxed);
                for button in notice.buttons {
                    probe.buttons.fetch_or(1 << button, Ordering::Relaxed);
                }
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    (
        server,
        credentials.remove(0),
        credentials.remove(0),
        cleanup,
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
async fn event(c: &mut NetworkClient) -> ClientEvent {
    tokio::time::timeout(Duration::from_secs(3), c.receive())
        .await
        .unwrap()
        .unwrap()
}
async fn applied(c: &mut NetworkClient, revision: u64, command_sequence: Option<u64>) {
    let mut state_applied = false;
    let mut result_received = command_sequence.is_none();
    for _ in 0..128 {
        match event(c).await {
            ClientEvent::Applied { revision: r } => {
                assert_eq!(r, revision);
                c.flush_pending().await.unwrap();
                assert!(!state_applied, "duplicate Applied event");
                state_applied = true;
            }
            ClientEvent::ViewOpened(v) => {
                assert_eq!(v.coverage, "complete");
                if c.view_id != [0; 16] && !v.granted_input_modes.is_empty() {
                    for mode in ["raw.pointer.v1", "text.client.v1", "resize.v1"] {
                        assert!(v.granted_input_modes.iter().any(|m| m == mode));
                    }
                }
            }
            ClientEvent::ResourcesNeeded { revision: r } => assert_eq!(r, revision),
            ClientEvent::ResourceAvailable => {}
            ClientEvent::CommandResult(r) => {
                assert_eq!(
                    Some(r.command_seq),
                    command_sequence,
                    "unexpected command result during publication"
                );
                assert!(!result_received, "duplicate command result");
                assert_eq!(r.status, "Applied");
                result_received = true;
            }
            other => panic!("unexpected public event {other:?}"),
        }
        if state_applied && result_received {
            return;
        }
    }
    panic!("bounded publication did not apply")
}
async fn result(c: &mut NetworkClient, sequence: u64, status: &str) {
    let received = event(c).await;
    let ClientEvent::CommandResult(r) = received else {
        panic!("expected result for sequence {sequence}, received public event {received:?}")
    };
    assert_eq!(r.command_seq, sequence);
    assert_eq!(r.status, status);
    assert!(!format!("{r:?}").contains(PREEDIT));
}
fn header(c: &NetworkClient, g: &sp::ControlGranted, seq: u64) -> p::CommandHeader {
    p::CommandHeader {
        epoch: c.epoch.to_vec(),
        view_id: c.view_id.to_vec(),
        client_instance_id: vec![3; 16],
        command_seq: seq,
        control_lease_id: g.lease_id.clone(),
        control_generation: g.generation,
        observed_view_revision: c.receiver().state().unwrap().revision,
    }
}
fn compose(
    c: &NetworkClient,
    g: &sp::ControlGranted,
    seq: u64,
    id: u8,
    target: u64,
    operation: p::text_composition::Operation,
) -> Vec<u8> {
    let state = c.receiver().state().unwrap();
    let focus = state
        .records
        .values()
        .find_map(|r| {
            if let Record::Session(s) = r {
                Some(s.focus_generation)
            } else {
                None
            }
        })
        .unwrap();
    let tid = if target == PUBLIC_TEXT_ID {
        PUBLIC_TEXT_MODEL_ID
    } else {
        SECRET_TEXT_ID
    };
    let Record::Text(t) = &state.records[&tid] else {
        panic!()
    };
    p::TextComposition {
        header: Some(header(c, g, seq)),
        target_id: target,
        text_session_id: vec![id; 16],
        focus_generation: focus,
        base_text_version: t.text_version,
        operation: Some(operation),
    }
    .encode_to_vec()
}
fn semantic(
    c: &NetworkClient,
    g: &sp::ControlGranted,
    seq: u64,
    target: u64,
    action: &str,
) -> Vec<u8> {
    let Record::Semantic(n) = &c.receiver().state().unwrap().records[&target] else {
        panic!()
    };
    let h = header(c, g, seq);
    p::SemanticCommand {
        epoch: h.epoch,
        view_id: h.view_id,
        client_instance_id: h.client_instance_id,
        command_seq: seq,
        control_lease_id: h.control_lease_id,
        control_generation: h.control_generation,
        target_id: target,
        action_id: action.into(),
        action_schema_version: 1,
        observed_view_revision: h.observed_view_revision,
        preconditions: Some(p::Preconditions {
            target_version: Some(n.version),
            ..Default::default()
        }),
        arguments: Some(p::semantic_command::Arguments::NoArguments(
            p::EmptyArguments {},
        )),
    }
    .encode_to_vec()
}
fn raw(c: &NetworkClient, g: &sp::ControlGranted, seq: u64, events: Vec<p::RawEvent>) -> Vec<u8> {
    let state = c.receiver().state().unwrap();
    let Record::Surface(s) = &state.records[&SURFACE_ID] else {
        panic!()
    };
    p::RawInputBatch {
        header: Some(header(c, g, seq)),
        surface_id: SURFACE_ID,
        routing_token: s.routing_token.clone(),
        presented_revision: state.revision,
        seat_id: FIXTURE_SEAT.into(),
        device_id: FIXTURE_DEVICE.into(),
        key_namespace: "linux.evdev".into(),
        events,
    }
    .encode_to_vec()
}
fn button(t: u64, pressed: bool, x: f64, y: f64) -> p::RawEvent {
    p::RawEvent {
        timestamp_us: t,
        event: Some(p::raw_event::Event::Button(p::ButtonEvent {
            code: 1,
            pressed,
            position: Some(p::Point { x, y }),
        })),
    }
}
async fn compare(server: &ServerHandle, c: &NetworkClient, o: &NetworkClient) {
    let host = server.host.lock().await;
    for (id, peer) in [(3, c), (4, o)] {
        let capture = host.source_capture(&client(id), peer.view_id).unwrap();
        assert_eq!(
            capture.state().records,
            peer.receiver().state().unwrap().records
        );
        assert!(
            host.retained_memory_bytes(&client(id), peer.view_id)
                .unwrap()
                <= peer.limits.view_memory_bytes
        );
        assert!(peer.receiver().memory_used() <= peer.limits.view_memory_bytes);
    }
    let Record::Text(t) = &o.receiver().state().unwrap().records[&SECRET_TEXT_ID] else {
        panic!()
    };
    assert_eq!(t.status, TextStatus::Redacted);
    assert!(t.content.is_none());
    for secret in [SECRET, PREEDIT] {
        let snapshot = host.snapshot_publication(&client(4), o.view_id).unwrap();
        assert!(!snapshot
            .windows(secret.len())
            .any(|w| w == secret.as_bytes()));
        assert!(!format!("{:?}", o.receiver().state().unwrap()).contains(secret));
    }
}
async fn apply_both(
    c: &mut NetworkClient,
    o: &mut NetworkClient,
    revision: u64,
    sequence: Option<u64>,
) {
    tokio::join!(applied(c, revision, sequence), applied(o, revision, None));
    c.presented(revision).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn tls_shared_sequence_composition_canvas_resize_and_private_source_are_coherent() {
    let (server, cc, oc, _cleanup) = setup().await;
    let mut c = connect(&server, &cc, 3).await;
    let mut o = connect(&server, &oc, 4).await;
    c.open_view().await.unwrap();
    o.open_view().await.unwrap();
    apply_both(&mut c, &mut o, 1, None).await;
    c.acquire_control([9; 16]).await.unwrap();
    let ClientEvent::ControlGranted(g) = event(&mut c).await else {
        panic!()
    };
    let begin = compose(
        &c,
        &g,
        1,
        7,
        PUBLIC_TEXT_ID,
        p::text_composition::Operation::Begin(p::ScalarRange { start: 1, end: 2 }),
    );
    c.submit_family(CommandFamily::TextComposition, &begin)
        .await
        .unwrap();
    result(&mut c, 1, "Dispatched").await;
    c.submit_family(CommandFamily::TextComposition, &begin)
        .await
        .unwrap();
    result(&mut c, 1, "Dispatched").await;
    let preedit = compose(
        &c,
        &g,
        2,
        7,
        PUBLIC_TEXT_ID,
        p::text_composition::Operation::UpdatePreedit(p::Preedit {
            text: "😀é".into(),
            selection: Some(p::ScalarRange { start: 0, end: 2 }),
        }),
    );
    c.submit_family(CommandFamily::TextComposition, &preedit)
        .await
        .unwrap();
    result(&mut c, 2, "Dispatched").await;
    assert_eq!(server.host.lock().await.diagnostics().source_commit, 1);
    let drag = raw(
        &c,
        &g,
        3,
        vec![
            button(1, true, 430.0, 200.0),
            p::RawEvent {
                timestamp_us: 2,
                event: Some(p::raw_event::Event::Motion(p::Point { x: 480.0, y: 230.0 })),
            },
            button(3, false, 490.0, 240.0),
        ],
    );
    c.submit_family(CommandFamily::RawInput, &drag)
        .await
        .unwrap();
    apply_both(&mut c, &mut o, 2, Some(3)).await;
    compare(&server, &c, &o).await;
    c.submit_family(CommandFamily::RawInput, &drag)
        .await
        .unwrap();
    result(&mut c, 3, "Applied").await;
    let Record::Scene(s) = &c.receiver().state().unwrap().records[&CANVAS_SHAPE_SCENE_ID] else {
        panic!()
    };
    let Primitive::SolidRect { rect, .. } = s.primitive else {
        panic!()
    };
    assert_eq!((rect.x, rect.y), (490.0, 240.0));
    let commit = compose(
        &c,
        &g,
        4,
        7,
        PUBLIC_TEXT_ID,
        p::text_composition::Operation::CommitText(p::CommitText {
            text: "😀é".into()
        }),
    );
    c.submit_family(CommandFamily::TextComposition, &commit)
        .await
        .unwrap();
    apply_both(&mut c, &mut o, 3, Some(4)).await;
    compare(&server, &c, &o).await;
    let Record::Text(t) = &c.receiver().state().unwrap().records[&PUBLIC_TEXT_MODEL_ID] else {
        panic!()
    };
    assert_eq!(t.content.as_deref(), Some("A😀éΩ אב"));
    let Record::Surface(s) = &c.receiver().state().unwrap().records[&SURFACE_ID] else {
        panic!()
    };
    let resize = p::ResizeRequest {
        header: Some(header(&c, &g, 5)),
        surface_id: SURFACE_ID,
        surface_version: s.version,
        width: 800.0,
        height: 600.0,
    }
    .encode_to_vec();
    c.submit_family(CommandFamily::Resize, &resize)
        .await
        .unwrap();
    apply_both(&mut c, &mut o, 4, Some(5)).await;
    compare(&server, &c, &o).await;
    let focus = semantic(&c, &g, 6, SECRET_ID, "Focus");
    c.submit(&focus).await.unwrap();
    apply_both(&mut c, &mut o, 5, Some(6)).await;
    let begin = compose(
        &c,
        &g,
        7,
        8,
        SECRET_ID,
        p::text_composition::Operation::Begin(p::ScalarRange { start: 0, end: 1 }),
    );
    c.submit_family(CommandFamily::TextComposition, &begin)
        .await
        .unwrap();
    result(&mut c, 7, "Dispatched").await;
    let preedit = compose(
        &c,
        &g,
        8,
        8,
        SECRET_ID,
        p::text_composition::Operation::UpdatePreedit(p::Preedit {
            text: PREEDIT.into(),
            selection: Some(p::ScalarRange {
                start: 0,
                end: PREEDIT.len() as u64,
            }),
        }),
    );
    c.submit_family(CommandFamily::TextComposition, &preedit)
        .await
        .unwrap();
    result(&mut c, 8, "Dispatched").await;
    compare(&server, &c, &o).await;
    let commit = compose(
        &c,
        &g,
        9,
        8,
        SECRET_ID,
        p::text_composition::Operation::CommitText(p::CommitText { text: "é".into() }),
    );
    c.submit_family(CommandFamily::TextComposition, &commit)
        .await
        .unwrap();
    apply_both(&mut c, &mut o, 6, Some(9)).await;
    compare(&server, &c, &o).await;
    assert_eq!(server.host.lock().await.diagnostics().consumed_commands, 9);
    assert_eq!(
        server
            .host
            .lock()
            .await
            .input_trace(&client(3), c.view_id)
            .unwrap()
            .len(),
        3
    );
    c.close().await.unwrap();
    o.close().await.unwrap();
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn tls_observer_stale_presentation_and_old_lease_cannot_mutate_source() {
    let (server, cc, oc, cleanup) = setup().await;
    let mut c = connect(&server, &cc, 3).await;
    let mut o = connect(&server, &oc, 4).await;
    c.open_view().await.unwrap();
    o.open_view().await.unwrap();
    apply_both(&mut c, &mut o, 1, None).await;
    c.acquire_control([9; 16]).await.unwrap();
    let ClientEvent::ControlGranted(g) = event(&mut c).await else {
        panic!()
    };
    let activate = semantic(&c, &g, 1, COUNTER_ID, "Activate");
    c.submit(&activate).await.unwrap();
    tokio::join!(applied(&mut c, 2, Some(1)), applied(&mut o, 2, None));
    // Applied alone is not proof of the new revision's actual presentation.
    let drag = raw(&c, &g, 2, vec![button(1, true, 430.0, 200.0)]);
    c.submit_family(CommandFamily::RawInput, &drag)
        .await
        .unwrap();
    result(&mut c, 2, "Rejected").await;
    c.presented(2).await.unwrap();
    let mut observer = p::RawInputBatch::decode(drag.as_slice()).unwrap();
    observer.header = Some(p::CommandHeader {
        view_id: o.view_id.to_vec(),
        client_instance_id: vec![4; 16],
        command_seq: 1,
        ..header(&c, &g, 1)
    });
    o.submit_family(CommandFamily::RawInput, &observer.encode_to_vec())
        .await
        .unwrap();
    result(&mut o, 1, "Rejected").await;
    let drag = raw(&c, &g, 3, vec![button(1, true, 430.0, 200.0)]);
    c.submit_family(CommandFamily::RawInput, &drag)
        .await
        .unwrap();
    apply_both(&mut c, &mut o, 3, Some(3)).await;
    server.host.lock().await.local_activity(10_000);
    let revoked = event(&mut c).await;
    assert!(
        matches!(revoked, ClientEvent::ControlRevoked(_)),
        "expected explicit takeover notification, got {revoked:?}"
    );
    let drag = raw(&c, &g, 4, vec![button(2, false, 440.0, 210.0)]);
    c.submit_family(CommandFamily::RawInput, &drag)
        .await
        .unwrap();
    result(&mut c, 4, "Rejected").await;
    let host = server.host.lock().await;
    assert_eq!(host.diagnostics().counter, 1);
    assert_eq!(host.diagnostics().source_commit, 3);
    assert!(host.input_trace(&client(3), c.view_id).unwrap().is_empty());
    drop(host);
    compare(&server, &c, &o).await;
    assert!(cleanup.count.load(Ordering::Relaxed) > 0);
    assert_eq!(cleanup.buttons.load(Ordering::Relaxed), 1 << 1);
    c.close().await.unwrap();
    o.close().await.unwrap();
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn tls_live_cache_loss_does_not_resume_held_drag_or_old_preedit_after_recovery() {
    let (server, cc, oc, _cleanup) = setup().await;
    let mut c = connect(&server, &cc, 3).await;
    let mut o = connect(&server, &oc, 4).await;
    c.open_view().await.unwrap();
    o.open_view().await.unwrap();
    apply_both(&mut c, &mut o, 1, None).await;
    c.acquire_control([9; 16]).await.unwrap();
    let ClientEvent::ControlGranted(g) = event(&mut c).await else {
        panic!()
    };
    let begin = compose(
        &c,
        &g,
        1,
        7,
        PUBLIC_TEXT_ID,
        p::text_composition::Operation::Begin(p::ScalarRange { start: 0, end: 0 }),
    );
    c.submit_family(CommandFamily::TextComposition, &begin)
        .await
        .unwrap();
    result(&mut c, 1, "Dispatched").await;
    let update = compose(
        &c,
        &g,
        2,
        7,
        PUBLIC_TEXT_ID,
        p::text_composition::Operation::UpdatePreedit(p::Preedit {
            text: "é".into(),
            selection: Some(p::ScalarRange { start: 0, end: 1 }),
        }),
    );
    c.submit_family(CommandFamily::TextComposition, &update)
        .await
        .unwrap();
    result(&mut c, 2, "Dispatched").await;
    let down = raw(&c, &g, 3, vec![button(1, true, 430.0, 200.0)]);
    c.submit_family(CommandFamily::RawInput, &down)
        .await
        .unwrap();
    apply_both(&mut c, &mut o, 2, Some(3)).await;
    c.resource_cache_lost().await.unwrap();
    assert_eq!(c.receiver().presented_revision().unwrap(), None);
    applied(&mut c, 2, None).await;
    assert_eq!(
        server
            .host
            .lock()
            .await
            .input_preedit(&client(3), c.view_id)
            .unwrap(),
        None
    );
    c.presented(2).await.unwrap();
    let update = compose(
        &c,
        &g,
        4,
        7,
        PUBLIC_TEXT_ID,
        p::text_composition::Operation::UpdatePreedit(p::Preedit {
            text: "stale".into(),
            selection: Some(p::ScalarRange { start: 0, end: 1 }),
        }),
    );
    c.submit_family(CommandFamily::TextComposition, &update)
        .await
        .unwrap();
    result(&mut c, 4, "Rejected").await;
    let release = raw(&c, &g, 5, vec![button(2, false, 480.0, 240.0)]);
    c.submit_family(CommandFamily::RawInput, &release)
        .await
        .unwrap();
    result(&mut c, 5, "Rejected").await;
    compare(&server, &c, &o).await;
    let host = server.host.lock().await;
    assert_eq!(host.diagnostics().source_commit, 2);
    assert_eq!(host.input_trace(&client(3), c.view_id).unwrap().len(), 1);
    drop(host);
    c.close().await.unwrap();
    o.close().await.unwrap();
    server.shutdown().await;
}
