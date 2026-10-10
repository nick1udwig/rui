//! Explicit native-font CPU/TLS benchmark, not a graphical Presented gate.
use prost::Message;
use rui_client_desktop::{RenderLimits, Renderer, Viewport};
use rui_host_broker::*;
use rui_protocol_core::{
    commands::AuthenticatedClient,
    model::{Limits, Replica},
    resources::MemoryBudget,
    session::*,
    transcript::{Direction, RecordingPolicy, TranscriptLimits},
    wire::proto as core,
};
use rui_transport::{
    diagnostics::Diagnostics, pairing::PairingAuthority, proto as p, ClientEvent, HostIdentity,
    NetworkClient, ServerConfig, ServerHandle,
};
use serde_json::json;
use std::{
    collections::BTreeSet,
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

fn auth(id: u8) -> AuthenticatedClient {
    AuthenticatedClient {
        principal: format!("load-{id}"),
        client_instance_id: [id; 16],
    }
}
fn hello(id: u8) -> ClientHello {
    let config = SessionConfig::default();
    let mut profiles = config.supported_profiles;
    profiles.insert("glyph.outline-single.v1".into());
    ClientHello {
        client_instance_id: [id; 16],
        supported_versions: [1].into_iter().collect(),
        supported_profiles: profiles,
        required_profiles: config.required_profiles,
        receiver_limits: ReceiverLimits {
            envelope_bytes: 65536,
            chunk_bytes: 32768,
            outbound_queue_bytes: 65536,
            ..Default::default()
        },
        resume: None,
    }
}
async fn applied(c: &mut NetworkClient, revision: u64) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match c.receive().await.unwrap() {
                ClientEvent::Applied { revision: r } => {
                    assert_eq!(r, revision);
                    c.flush_pending().await.unwrap();
                    break;
                }
                ClientEvent::ViewOpened(v) => {
                    assert_eq!(v.canvas_width, PERFORMANCE_WIDTH);
                    assert_eq!(v.coverage, "complete");
                }
                ClientEvent::ResourcesNeeded { revision: r } => assert_eq!(r, revision),
                ClientEvent::ResourceAvailable => {}
                other => panic!("unexpected load event {other:?}"),
            }
        }
    })
    .await
    .unwrap();
}
async fn barrier(c: &mut NetworkClient, nonce: u64) {
    c.send(p::Kind::Ping, &p::Ping { nonce }).await.unwrap();
    assert!(matches!(c.receive().await.unwrap(),ClientEvent::Pong(n) if n==nonce));
}
async fn recovered(c: &mut NetworkClient, revision: Option<u64>) -> (bool, usize) {
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut resumed = false;
        let mut needs = 0;
        loop {
            match c.receive().await.unwrap() {
                ClientEvent::ResumeAccepted(_) => resumed = true,
                ClientEvent::ViewOpened(_) | ClientEvent::ResourceAvailable => {}
                ClientEvent::ResourcesNeeded { revision: r } => {
                    if let Some(expected) = revision {
                        assert_eq!(r, expected);
                    }
                    needs += 1;
                }
                ClientEvent::Applied { revision: r } => {
                    if let Some(expected) = revision {
                        assert_eq!(r, expected);
                    }
                    c.flush_pending().await.unwrap();
                    return (resumed, needs);
                }
                other => panic!("unexpected recovery event {other:?}"),
            }
        }
    })
    .await
    .unwrap()
}
fn traffic(diagnostics: &Diagnostics, view: [u8; 16]) -> (usize, usize) {
    let scope = diagnostics
        .scopes()
        .unwrap()
        .into_iter()
        .find(|s| s.view_id == view)
        .unwrap();
    diagnostics
        .inspect(scope, |entries| {
            entries.fold((0, 0), |(bytes, bodies), entry| {
                (
                    bytes + entry.wire_bytes,
                    bodies + usize::from(entry.kind == p::Kind::ResourceComplete as u32),
                )
            })
        })
        .unwrap()
}
fn summary(mut samples: Vec<u128>) -> serde_json::Value {
    samples.sort_unstable();
    let at = |p: usize| samples[(samples.len() * p).div_ceil(100).saturating_sub(1)];
    json!({"unit":"microseconds","samples":samples.len(),"p50":at(50),"p95":at(95),"p99":at(99)})
}
#[derive(Default)]
struct Samples {
    publish: Vec<u128>,
    network: Vec<u128>,
    render: Vec<u128>,
    encode: Vec<u128>,
    apply: Vec<u128>,
    bytes: Vec<usize>,
    first_applied: Vec<u128>,
    second_applied: Vec<u128>,
    first_render: Vec<u128>,
    second_render: Vec<u128>,
    first_cpu_ready: Vec<u128>,
    second_cpu_ready: Vec<u128>,
}
#[tokio::main]
async fn main() {
    let recovery_only = std::env::args().nth(1).as_deref() == Some("--recovery-only");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let provider = NativeSourceFontProvider::new(
        "/usr/bin/python3".into(),
        root.join("fixtures/render/native_font.py"),
    );
    let host = ControlledHost::new_performance(
        HostConfig {
            max_text_bytes: 64,
            session: SessionConfig {
                max_history_records: 32,
                ..Default::default()
            },
            ..Default::default()
        },
        &provider,
    )
    .unwrap();
    let mut pairing = PairingAuthority::new(3).unwrap();
    let mut credentials = Vec::new();
    for id in 3..=5 {
        let request = pairing.request_local_pairing([id; 16]).unwrap();
        credentials.push(
            pairing
                .approve_local(
                    request.request_id,
                    VerifiedAuthorization {
                        principal: auth(id).principal,
                        policy_version: 1,
                        permissions: Permissions {
                            can_view: true,
                            can_control: false,
                            actions: Default::default(),
                        },
                    },
                )
                .unwrap(),
        );
    }
    let diagnostics = Diagnostics::new(
        RecordingPolicy::AuthorizedDiagnosticBytes,
        TranscriptLimits {
            records: 8192,
            retained_bytes: 16 << 20,
            ..Default::default()
        },
        3,
    )
    .unwrap();
    let server = ServerHandle::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        HostIdentity::from_der(
            include_bytes!("../tests/data/localhost.der").to_vec(),
            include_bytes!("../tests/data/localhost-key.der").to_vec(),
        )
        .unwrap(),
        host,
        Arc::new(Mutex::new(pairing)),
        ServerConfig {
            max_message_bytes: 65536,
            diagnostics: Some(diagnostics.clone()),
            publication_timeout: Duration::from_secs(300),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut clients = Vec::new();
    let mut cold = Vec::new();
    for (i, credential) in credentials.iter().enumerate() {
        let connecting = Instant::now();
        let mut c = NetworkClient::connect(
            server.address,
            "localhost".into(),
            include_bytes!("../tests/data/localhost.der").to_vec(),
            credential,
            hello(i as u8 + 3),
        )
        .await
        .unwrap();
        c.open_view().await.unwrap();
        applied(&mut c, 1).await;
        let applied_us = connecting.elapsed().as_micros();
        barrier(&mut c, 1).await;
        let (wire_bytes, bodies) = traffic(&diagnostics, c.view_id);
        cold.push(json!({"client":i + 3,"through_applied_us":applied_us,"scoped_plaintext_wire_bytes":wire_bytes,"resource_bodies":bodies,"receiver_peak_bytes":c.receiver().memory_budget().peak()}));
        assert_eq!(
            c.receiver().state().unwrap().records.len(),
            PERFORMANCE_RECORDS
        );
        clients.push(c);
    }
    let slow = clients.pop().unwrap(); // Remains connected but stops consuming.
    let [mut first, mut second]: [NetworkClient; 2] = clients.try_into().ok().unwrap();
    let scope = diagnostics
        .scopes()
        .unwrap()
        .into_iter()
        .find(|s| s.view_id == first.view_id)
        .unwrap();
    let baseline = server
        .host
        .lock()
        .await
        .snapshot_publication(&auth(3), first.view_id)
        .unwrap();
    let envelope = core::Envelope::decode(baseline.as_ref()).unwrap();
    let snapshot = core::Snapshot::decode(envelope.payload.as_slice()).unwrap();
    let resources: BTreeSet<[u8; 32]> = snapshot
        .required_resources
        .iter()
        .map(|d| d.as_slice().try_into().unwrap())
        .collect();
    assert_eq!(resources.len(), 1);
    let replay_budget = MemoryBudget::new(128 << 20);
    let mut replay = Replica::with_memory_budget(Limits::default(), replay_budget.clone());
    replay
        .apply_snapshot(&envelope.payload, &resources)
        .unwrap();
    let snapshot_bytes = baseline.len();
    drop(baseline);
    let render_budget = MemoryBudget::new(256 << 20);
    let renderer = Renderer::new(RenderLimits::default(), render_budget.clone()).unwrap();
    let viewport = Viewport::new(PERFORMANCE_WIDTH as u32, PERFORMANCE_HEIGHT as u32);
    let initial_cache = [
        first.receiver().cache_bytes().unwrap(),
        second.receiver().cache_bytes().unwrap(),
    ];
    let mut peak_host = 0;
    let mut peak_client = 0;
    let mut peak_queue = 0;
    let mut peak_renderer = 0;
    let mut rows = Vec::new();
    let start = Instant::now();
    let mut revision = 1;
    for count in [1usize, 20].into_iter().filter(|_| !recovery_only) {
        let mut samples = Samples::default();
        for iteration in 0..110 {
            if iteration % 25 == 0 {
                eprintln!("load: {count} changed records, iteration {iteration}/110");
            }
            revision += 1;
            let change = Instant::now();
            server
                .host
                .lock()
                .await
                .advance_performance(count, start.elapsed().as_millis() as u64)
                .unwrap();
            let publication_us = change.elapsed().as_micros();
            let (first_applied_us, second_applied_us) = tokio::join!(
                async {
                    applied(&mut first, revision).await;
                    change.elapsed().as_micros()
                },
                async {
                    applied(&mut second, revision).await;
                    change.elapsed().as_micros()
                }
            );
            let receive_us = change.elapsed().as_micros();
            let rendering = Instant::now();
            let frame = renderer
                .render_view(
                    first.receiver().state().unwrap(),
                    viewport,
                    first.receiver(),
                )
                .unwrap();
            let first_render_us = rendering.elapsed().as_micros();
            let first_cpu_ready_us = change.elapsed().as_micros();
            peak_renderer = peak_renderer.max(render_budget.peak());
            let second_rendering = Instant::now();
            let second_frame = renderer
                .render_view(
                    second.receiver().state().unwrap(),
                    viewport,
                    second.receiver(),
                )
                .unwrap();
            let second_render_us = second_rendering.elapsed().as_micros();
            let second_cpu_ready_us = change.elapsed().as_micros();
            assert_eq!(frame.pixels(), second_frame.pixels());
            peak_renderer = peak_renderer.max(render_budget.peak());
            let render_us = rendering.elapsed().as_micros();
            drop(frame);
            drop(second_frame);
            assert_eq!(
                first.receiver().state().unwrap().records,
                second.receiver().state().unwrap().records
            );
            assert_eq!(
                initial_cache,
                [
                    first.receiver().cache_bytes().unwrap(),
                    second.receiver().cache_bytes().unwrap()
                ]
            );
            let host = server.host.lock().await;
            for (id, c) in [(3, &first), (4, &second)] {
                let memory = host.retained_memory_bytes(&auth(id), c.view_id).unwrap();
                assert!(memory <= c.limits.view_memory_bytes);
                peak_host = peak_host.max(host.peak_memory_bytes(&auth(id), c.view_id).unwrap());
                peak_client = peak_client.max(c.receiver().memory_budget().peak());
                let queue = host.queued_bytes(&auth(id), c.view_id).unwrap();
                assert!(queue <= c.limits.outbound_queue_bytes);
                peak_queue = peak_queue.max(queue);
            }
            if let Ok(queue) = host.queued_bytes(&auth(5), slow.view_id) {
                assert!(queue <= slow.limits.outbound_queue_bytes);
                peak_queue = peak_queue.max(queue);
            }
            drop(host);
            // Decode the exact pre-encryption publication produced for client 3.
            // The diagnostic owner itself is charged to that view throughout.
            let bytes = diagnostics
                .inspect(scope, |entries| {
                    let mut id = None;
                    let mut body = Vec::new();
                    for entry in entries.filter(|e| e.direction == Direction::HostToClient) {
                        let packet = p::Packet::decode(entry.exact_bytes().unwrap()).unwrap();
                        if packet.kind == p::Kind::TransactionBegin as i32 {
                            let begin =
                                p::PublicationBegin::decode(packet.payload.as_slice()).unwrap();
                            if begin.revision == revision {
                                assert_eq!(begin.item_count as usize, count);
                                id = Some(begin.publication_id);
                            }
                        } else if packet.kind == p::Kind::PublicationChunk as i32 {
                            let chunk =
                                p::PublicationChunk::decode(packet.payload.as_slice()).unwrap();
                            if id.as_ref() == Some(&chunk.publication_id) {
                                body.extend(chunk.data);
                            }
                        }
                    }
                    body
                })
                .unwrap();
            assert!(bytes.len() < snapshot_bytes / 20);
            let tx = core::Transaction::decode(bytes.as_slice()).unwrap();
            assert_eq!(tx.mutations.len(), count);
            let encoding = Instant::now();
            let encoded = tx.encode_to_vec();
            let encode_us = encoding.elapsed().as_micros();
            let applying = Instant::now();
            replay.apply_transaction(&encoded, &resources).unwrap();
            let apply_us = applying.elapsed().as_micros();
            assert_eq!(
                replay.state().records,
                first.receiver().state().unwrap().records
            );
            if iteration >= 10 {
                samples.publish.push(publication_us);
                samples.network.push(receive_us);
                samples.render.push(render_us);
                samples.encode.push(encode_us);
                samples.apply.push(apply_us);
                samples.bytes.push(bytes.len());
                samples.first_applied.push(first_applied_us);
                samples.second_applied.push(second_applied_us);
                samples.first_render.push(first_render_us);
                samples.second_render.push(second_render_us);
                samples.first_cpu_ready.push(first_cpu_ready_us);
                samples.second_cpu_ready.push(second_cpu_ready_us);
            }
        }
        let raw = json!({"publication":samples.publish,"two_applied":samples.network,"two_renders_and_comparison":samples.render,"encode":samples.encode,"independent_apply":samples.apply,"bytes":samples.bytes,"first_applied":samples.first_applied,"second_applied":samples.second_applied,"first_render":samples.first_render,"second_render":samples.second_render,"first_cpu_ready":samples.first_cpu_ready,"second_cpu_ready":samples.second_cpu_ready});
        rows.push(json!({"changed_records":count,"publication_all_views":summary(samples.publish),"source_change_to_two_applied":summary(samples.network),"two_cpu_renders":summary(samples.render),"transaction_encode":summary(samples.encode),"independent_apply":summary(samples.apply),"publication_bytes_min":samples.bytes.iter().min(),"publication_bytes_max":samples.bytes.iter().max(),"per_client":[{"applied":summary(samples.first_applied),"cpu_render":summary(samples.first_render),"source_change_to_cpu_frame":summary(samples.first_cpu_ready)},{"applied":summary(samples.second_applied),"cpu_render":summary(samples.second_render),"source_change_to_cpu_frame":summary(samples.second_cpu_ready)}],"raw_sample_units":"microseconds except bytes","raw_samples":raw}));
    }
    barrier(&mut first, 2).await;
    let (resource_bodies, wire_bytes) = diagnostics
        .inspect(scope, |entries| {
            let mut resource_bodies = 0;
            let mut wire_bytes = 0;
            for entry in entries {
                wire_bytes += entry.wire_bytes;
                if entry.kind == p::Kind::ResourceComplete as u32 {
                    resource_bodies += 1;
                }
            }
            (resource_bodies, wire_bytes)
        })
        .unwrap();
    assert_eq!(
        resource_bodies, 1,
        "warm updates must not resend the cached outline"
    );
    let slow_status = server
        .host
        .lock()
        .await
        .view_status(&auth(5), slow.view_id)
        .unwrap();
    assert!(
        recovery_only || slow_status.closed.is_some(),
        "bounded stalled observer must close independently"
    );
    let capture = server
        .host
        .lock()
        .await
        .source_capture(&auth(3), first.view_id)
        .unwrap();
    assert_eq!(
        capture.state().records,
        first.receiver().state().unwrap().records
    );
    struct SourceResources<'a>(&'a SourceCapture);
    impl rui_client_desktop::ResourceProvider for SourceResources<'_> {
        fn get(&self, digest: &[u8; 32]) -> Option<rui_client_desktop::ResourceView<'_>> {
            self.0
                .resources()
                .find(|(offer, _)| &offer.digest == digest)
                .map(|(offer, bytes)| rui_client_desktop::ResourceView { offer, bytes })
        }
    }
    let direct = renderer
        .render_view(capture.state(), viewport, &SourceResources(&capture))
        .unwrap();
    let replayed = renderer
        .render_view(
            first.receiver().state().unwrap(),
            viewport,
            first.receiver(),
        )
        .unwrap();
    assert_eq!(direct.pixels(), replayed.pixels());
    assert!(direct.pixels().iter().any(|p| p.alpha > 0.0));
    drop(direct);
    drop(replayed);
    drop(capture);
    peak_host = peak_host.max(
        server
            .host
            .lock()
            .await
            .peak_memory_bytes(&auth(3), first.view_id)
            .unwrap(),
    );
    peak_renderer = peak_renderer.max(render_budget.peak());
    let idle_sequence = diagnostics
        .inspect(scope, |entries| entries.count())
        .unwrap();
    let before = server.host.lock().await.diagnostics().source_commit;
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(server.host.lock().await.diagnostics().source_commit, before);
    assert_eq!(
        diagnostics
            .inspect(scope, |entries| entries.count())
            .unwrap(),
        idle_sequence
    );
    // Recovery follows the frozen warm workload and its resource/idle checks.
    // A narrower envelope forces a fresh scope without raising any ceiling.
    let mut recovery = Vec::new();
    for mode in [
        "retained_resume",
        "retained_cache_loss",
        "forced_fresh_scope",
    ] {
        let old_view = first.view_id;
        let connecting = Instant::now();
        let mut retained = first.into_retained();
        if mode == "retained_cache_loss" {
            retained.lose_resource_cache().unwrap();
        }
        let mut negotiation = hello(3);
        if mode == "forced_fresh_scope" {
            negotiation.receiver_limits.envelope_bytes = 49152;
        }
        first = retained
            .reconnect(
                server.address,
                "localhost".into(),
                include_bytes!("../tests/data/localhost.der").to_vec(),
                &credentials[0],
                negotiation,
            )
            .await
            .unwrap();
        first.open_view().await.unwrap();
        let (resumed, needs) = recovered(
            &mut first,
            (mode != "forced_fresh_scope").then_some(revision),
        )
        .await;
        let applied_us = connecting.elapsed().as_micros();
        assert_eq!(resumed, mode != "forced_fresh_scope");
        assert_eq!(first.view_id == old_view, mode != "forced_fresh_scope");
        assert_eq!(first.commands().len(), 0);
        assert_eq!(first.receiver().presented_revision().unwrap(), None);
        let rendering = Instant::now();
        let frame = renderer
            .render_view(
                first.receiver().state().unwrap(),
                viewport,
                first.receiver(),
            )
            .unwrap();
        let render_us = rendering.elapsed().as_micros();
        let cpu_ready_us = connecting.elapsed().as_micros();
        let comparison = renderer
            .render_view(
                second.receiver().state().unwrap(),
                viewport,
                second.receiver(),
            )
            .unwrap();
        assert_eq!(frame.pixels(), comparison.pixels());
        drop(frame);
        drop(comparison);
        if mode != "forced_fresh_scope" {
            assert_eq!(
                first.receiver().state().unwrap().records,
                second.receiver().state().unwrap().records
            );
        }
        // Fresh scopes restart public clocks. Compare their exact records to
        // their own source projection, and visual content to the healthy peer.
        let capture = server
            .host
            .lock()
            .await
            .source_capture(&auth(3), first.view_id)
            .unwrap();
        assert_eq!(
            capture.state().records,
            first.receiver().state().unwrap().records
        );
        drop(capture);
        barrier(&mut first, 3).await;
        let after = traffic(&diagnostics, first.view_id);
        // Diagnostic buffers are connection-owned and disappear on socket
        // closure, including when the resumed view ID is unchanged.
        let bodies = after.1;
        assert_eq!(bodies, usize::from(mode != "retained_resume"));
        let receiver_peak = first.receiver().memory_budget().peak();
        let host = server.host.lock().await;
        let host_peak = host.peak_memory_bytes(&auth(3), first.view_id).unwrap();
        assert!(receiver_peak <= first.limits.view_memory_bytes);
        assert!(host_peak <= first.limits.view_memory_bytes);
        assert!(render_budget.peak() <= 256 << 20);
        recovery.push(json!({"mode":mode,"through_applied_us":applied_us,"cpu_render_us":render_us,"through_cpu_frame_us":cpu_ready_us,"scoped_plaintext_wire_bytes":after.0,"resource_bodies":bodies,"resource_wait_events":needs,"receiver_peak_bytes":receiver_peak,"host_peak_bytes":host_peak,"renderer_peak_bytes":render_budget.peak(),"pending_commands":0,"presented":false,"same_view":first.view_id == old_view}));
    }
    let rss = std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|line| line.starts_with("VmHWM:"))
        .unwrap()
        .to_owned();
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .unwrap()
        .lines()
        .find(|line| line.starts_with("model name"))
        .unwrap()
        .to_owned();
    drop(replay);
    drop(renderer);
    assert_eq!(replay_budget.used(), 0);
    assert_eq!(render_budget.used(), 0);
    first.close().await.unwrap();
    second.close().await.unwrap();
    drop(slow);
    server.shutdown().await;
    assert!(diagnostics.scopes().unwrap().is_empty());
    println!("{}",serde_json::to_string_pretty(&json!({"recovery_only":recovery_only,"cold":cold,"recovery":recovery,"recovery_byte_boundary":"per-view plaintext envelopes including ACK/barrier; excludes authentication negotiation and TLS/socket overhead","stage_timings":rui_protocol_core::timing::inspect(|t| serde_json::to_value(t).unwrap()),"hardware":cpu,"whole_process_peak_rss":rss,"profile":"release CPU/TLS, pinned native Pango/Cairo glyph outline","records":PERFORMANCE_RECORDS,"visible_glyph_cells":PERFORMANCE_GLYPHS,"clients":"two healthy plus one stalled observer","snapshot_bytes":snapshot_bytes,"updates":rows,"one_client_total_plaintext_wire_bytes":wire_bytes,"resource_bodies_cold":resource_bodies,"resource_bodies_warm":0,"warm_resource_cache_hit_rate":1.0,"peak_host_view_bytes":peak_host,"peak_receiver_bytes":peak_client,"sampled_outbound_queue_bytes":peak_queue,"peak_renderer_bytes":peak_renderer,"slow_view_closed":format!("{:?}",slow_status.closed),"released_replay_and_renderer_bytes":0,"idle_publication_bytes_250ms":0,"elapsed_seconds":start.elapsed().as_secs_f64(),"presentation":"CPU frames only; no native Presented or action-to-present latency claim; native GUI gate supplies that separately"})).unwrap());
}
