//! Deterministic complete application-packet cuts over actual pinned TLS.
//! CPU comparisons use direct authoritative records and recovered replicas;
//! protocol Presented acknowledgements below are simulated fixture readiness.
#[path = "common/fault_proxy.rs"]
mod proxy;
use prost::Message;
use proxy::{Plan, Proxy, CEILING};
use rui_client_desktop::{RenderLimits, Renderer, Viewport};
use rui_host_broker::*;
use rui_protocol_core::{
    commands::AuthenticatedClient, resources::*, session::*, wire::proto as core,
};
use rui_transport::{
    pairing::{Credential, PairingAuthority},
    proto as p, ClientEvent, HostIdentity, NetworkClient, ServerConfig, ServerHandle,
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};

struct SourceResources<'a>(&'a SourceCapture);
impl rui_client_desktop::ResourceProvider for SourceResources<'_> {
    fn get(&self, digest: &[u8; 32]) -> Option<rui_client_desktop::ResourceView<'_>> {
        self.0
            .resources()
            .find(|(offer, _)| &offer.digest == digest)
            .map(|(offer, bytes)| rui_client_desktop::ResourceView { offer, bytes })
    }
}

struct VectorFont;
impl SourceFontProvider for VectorFont {
    fn shape(&self, _text: &str, size: f64) -> Result<SourceShapedText, SourceFontError> {
        // Synthetic authored outline: enough exact bytes for three resource
        // chunks, with small bounded geometry. No native font fidelity claim.
        let mut body = b"RVP1".to_vec();
        body.extend_from_slice(&502u32.to_be_bytes());
        body.push(1);
        body.extend_from_slice(&0f64.to_be_bytes());
        body.extend_from_slice(&0f64.to_be_bytes());
        for i in 0..500 {
            body.push(2);
            body.extend_from_slice(&(f64::from(i % 2) * 0.5).to_be_bytes());
            body.extend_from_slice(&(f64::from((i / 2) % 2) * 0.5).to_be_bytes());
        }
        body.push(5);
        let digest = resource_digest(
            ResourceKind::GlyphOutline,
            1,
            ResourceProvenance::FontOutline,
            &body,
        );
        Ok(SourceShapedText {
            glyphs: vec![SourceGlyph {
                resource: digest,
                size,
                position: [0.0, 12.0],
            }],
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
fn authenticated() -> AuthenticatedClient {
    AuthenticatedClient {
        principal: "fault-controller".into(),
        client_instance_id: [3; 16],
    }
}
fn hello() -> ClientHello {
    let mut h = ClientHello {
        client_instance_id: [3; 16],
        supported_versions: [1].into_iter().collect(),
        supported_profiles: SessionConfig::default().supported_profiles,
        required_profiles: SessionConfig::default().required_profiles,
        receiver_limits: ReceiverLimits {
            envelope_bytes: CEILING,
            chunk_bytes: 4096,
            ..Default::default()
        },
        resume: None,
    };
    h.supported_profiles
        .insert("glyph.outline-single.v1".into());
    h
}
async fn setup() -> (ServerHandle, Credential) {
    let mut pairing = PairingAuthority::new(1).unwrap();
    let request = pairing.request_local_pairing([3; 16]).unwrap();
    let credential = pairing
        .approve_local(
            request.request_id,
            VerifiedAuthorization {
                principal: authenticated().principal,
                policy_version: 1,
                permissions: Permissions {
                    can_view: true,
                    can_control: true,
                    actions: ["Activate".into()].into_iter().collect(),
                },
            },
        )
        .unwrap();
    let host = ControlledHost::new_corpus_with_fonts(
        HostConfig::default(),
        "FAULT-PRIVATE-SENTINEL".into(),
        Arc::new(VectorFont),
    )
    .unwrap();
    let identity = HostIdentity::from_der(
        include_bytes!("data/localhost.der").to_vec(),
        include_bytes!("data/localhost-key.der").to_vec(),
    )
    .unwrap();
    let server = ServerHandle::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        identity,
        host,
        Arc::new(Mutex::new(pairing)),
        ServerConfig {
            max_message_bytes: CEILING,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    (server, credential)
}
async fn connect(address: SocketAddr, credential: &Credential) -> NetworkClient {
    NetworkClient::connect(
        address,
        "localhost".into(),
        include_bytes!("data/localhost.der").to_vec(),
        credential,
        hello(),
    )
    .await
    .unwrap()
}
async fn applied(client: &mut NetworkClient, revision: u64) {
    tokio::time::timeout(Duration::from_secs(5),async {
        loop {if matches!(client.receive().await.unwrap(),ClientEvent::Applied{revision:r} if r>=revision){client.flush_pending().await.unwrap();break}}
    }).await.unwrap();
}
async fn offline(server: &ServerHandle, view: [u8; 16]) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while server
            .host
            .lock()
            .await
            .view_status(&authenticated(), view)
            .unwrap()
            .connected
        {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}
fn activate(client: &NetworkClient, lease: &p::ControlGranted) -> Vec<u8> {
    core::SemanticCommand {
        epoch: client.epoch.to_vec(),
        view_id: client.view_id.to_vec(),
        client_instance_id: [3; 16].to_vec(),
        command_seq: 1,
        control_lease_id: lease.lease_id.clone(),
        control_generation: lease.generation,
        target_id: COUNTER_ID,
        action_id: "Activate".into(),
        action_schema_version: 1,
        observed_view_revision: 1,
        preconditions: None,
        arguments: Some(core::semantic_command::Arguments::NoArguments(
            core::EmptyArguments {},
        )),
    }
    .encode_to_vec()
}
async fn compare(server: &ServerHandle, client: &NetworkClient) {
    let capture = server
        .host
        .lock()
        .await
        .source_capture(&authenticated(), client.view_id)
        .unwrap();
    assert_eq!(
        capture.state().records,
        client.receiver().state().unwrap().records
    );
    let budget = MemoryBudget::new(64 << 20);
    let renderer = Renderer::new(RenderLimits::default(), budget.clone()).unwrap();
    let mut viewport = Viewport::new(460, 240);
    viewport.scale = 0.5; // Entire 920x480 authoritative canvas, not a crop.
    let canvas = capture
        .state()
        .records
        .values()
        .find_map(|record| match record {
            rui_protocol_core::model::Record::Session(session) => Some(session.canvas),
            _ => None,
        })
        .unwrap();
    assert_eq!((canvas.width, canvas.height), (920.0, 480.0));
    let direct = renderer
        .render_view(capture.state(), viewport, &SourceResources(&capture))
        .unwrap();
    let replay = renderer
        .render_view(
            client.receiver().state().unwrap(),
            viewport,
            client.receiver(),
        )
        .unwrap();
    assert_eq!(direct.pixels(), replay.pixels());
    assert!(client.receiver().memory_used() <= client.limits.view_memory_bytes);
}
async fn exercise(transaction: bool, cut: Option<usize>, delay: Duration) -> Vec<p::Kind> {
    let (server, credential) = setup().await;
    let proxy = Proxy::start(
        server.address,
        Plan {
            remaining: cut,
            armed: !transaction,
            delay,
            ..Default::default()
        },
    )
    .await;
    let mut client = connect(proxy.address, &credential).await;
    client.open_view().await.unwrap();
    let mut command = None;
    if transaction {
        applied(&mut client, 1).await;
        client.presented(1).await.unwrap();
        client.acquire_control([8; 16]).await.unwrap();
        let ClientEvent::ControlGranted(lease) = client.receive().await.unwrap() else {
            panic!()
        };
        proxy.plan.lock().unwrap().armed = true;
        let exact = activate(&client, &lease);
        client.submit(&exact).await.unwrap();
        command = Some(exact);
    }
    let revision = if transaction { 2 } else { 1 };
    if cut.is_some() {
        let prior = client.receiver().state().unwrap();
        let prior_memory = client
            .receiver()
            .memory_budget()
            .reserve(prior.estimated_bytes() + 4096)
            .unwrap();
        let prior_records = prior.records.clone();
        tokio::time::timeout(Duration::from_secs(5), async {
            while client.receive().await.is_ok() {}
        })
        .await
        .unwrap();
        assert!(proxy.plan.lock().unwrap().cut);
        assert_eq!(
            client.receiver().state().unwrap().revision,
            if transaction { 1 } else { 0 }
        );
        assert_eq!(client.receiver().state().unwrap().records, prior_records);
        drop(prior_records);
        drop(prior_memory);
        let view = client.view_id;
        let budget = client.receiver().memory_budget();
        let mut retained = client.into_retained();
        assert_eq!(retained.receiver().unwrap().staged_publication_bytes(), 0);
        assert!(!retained.receiver().unwrap().has_pending_publication());
        offline(&server, view).await;
        client = retained
            .reconnect(
                server.address,
                "localhost".into(),
                include_bytes!("data/localhost.der").to_vec(),
                &credential,
                hello(),
            )
            .await
            .unwrap();
        client.open_view().await.unwrap();
        applied(&mut client, revision).await;
        assert!(retained.receiver().is_none());
        assert!(budget.used() <= budget.limit());
        assert!(client.receiver().presented_revision().unwrap().is_none());
    } else {
        applied(&mut client, revision).await;
    }
    proxy.plan.lock().unwrap().armed = false;
    compare(&server, &client).await;
    if let Some(command) = command {
        client.submit(&command).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let ClientEvent::CommandResult(result) = client.receive().await.unwrap() {
                    assert_eq!(result.status, "Applied");
                    break;
                }
            }
        })
        .await
        .unwrap();
        let diagnostics = server.host.lock().await.diagnostics();
        assert_eq!(diagnostics.counter, 1);
        assert_eq!(diagnostics.consumed_commands, 1);
        if cut.is_some() {
            let mut stale = core::SemanticCommand::decode(command.as_slice()).unwrap();
            stale.command_seq = 2;
            client.submit(&stale.encode_to_vec()).await.unwrap();
            let ClientEvent::CommandResult(result) = client.receive().await.unwrap() else {
                panic!()
            };
            assert_eq!(result.status, "Rejected");
            assert_eq!(server.host.lock().await.diagnostics().counter, 1);
        }
    }
    let seen = proxy.plan.lock().unwrap().seen.clone();
    let budget = client.receiver().memory_budget();
    client.close().await.unwrap();
    server.shutdown().await;
    assert_eq!(budget.used(), 0);
    seen
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn every_actual_tls_publication_and_resource_boundary_recovers_exact_state_and_pixels() {
    for transaction in [false, true] {
        let trace = exercise(transaction, None, Duration::ZERO).await;
        assert!(trace.contains(&p::Kind::PublicationChunk));
        assert!(trace.contains(&p::Kind::PublicationCommit));
        if !transaction {
            assert!(
                trace
                    .iter()
                    .filter(|k| **k == p::Kind::ResourceChunk)
                    .count()
                    >= 3
            );
        }
        assert!(trace.len() < 128);
        for cut in 0..trace.len() {
            exercise(transaction, Some(cut), Duration::ZERO).await;
        }
        // Ordered delivery under delay is a separate successful run.
        exercise(transaction, None, Duration::from_millis(3)).await;
        eprintln!(
            "actual TLS {}: {} cut points plus delayed recovery",
            if transaction {
                "delta"
            } else {
                "snapshot/resources"
            },
            trace.len()
        );
    }
}
