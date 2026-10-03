//! Sustained real TLS surface/resource/view/lease churn under explicit ceilings.
//! Presented messages here are protocol readiness fixtures, not GUI evidence.
#[path = "common/resources.rs"]
#[allow(dead_code)]
mod fixture;
use fixture::*;
use prost::Message;
use rui_host_broker::{ControlledHost, HostConfig, POPUP_TOGGLE_ID};
use rui_protocol_core::{model::Record, session::*, wire::proto as core};
use rui_transport::{
    pairing::PairingAuthority, proto as p, ClientEvent, HostIdentity, NetworkClient, ServerConfig,
    ServerHandle,
};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};

async fn event(c: &mut NetworkClient) -> ClientEvent {
    tokio::time::timeout(Duration::from_secs(5), c.receive())
        .await
        .unwrap()
        .unwrap()
}
async fn pong(c: &mut NetworkClient, n: u64) {
    c.send(p::Kind::Ping, &p::Ping { nonce: n }).await.unwrap();
    assert!(matches!(event(c).await,ClientEvent::Pong(v) if v==n));
}
async fn effect(c: &mut NetworkClient, revision: u64, sequence: u64) -> p::CommandResult {
    let mut result = None;
    let mut committed = false;
    for _ in 0..1024 {
        match event(c).await {
            ClientEvent::CommandResult(r) => {
                assert_eq!(r.command_seq, sequence);
                assert_eq!(r.status, "Applied");
                assert!(result.replace(r).is_none());
            }
            ClientEvent::Applied { revision: r } => {
                assert_eq!(r, revision);
                assert!(!committed);
                committed = true;
            }
            ClientEvent::ResourcesNeeded { revision: r } => assert_eq!(r, revision),
            ClientEvent::ResourceAvailable => {}
            other => panic!("unexpected public churn event {other:?}"),
        }
        if committed {
            if let Some(result) = result.take() {
                c.flush_pending().await.unwrap();
                return result;
            }
        }
    }
    panic!("churn publication bound")
}
async fn recover(
    server: &ServerHandle,
    c: NetworkClient,
    credential: &rui_transport::pairing::Credential,
    id: u8,
    revision: u64,
    lose_cache: bool,
) -> NetworkClient {
    let view = c.view_id;
    let mut retained = c.into_retained();
    assert!(retained
        .receiver()
        .unwrap()
        .presented_revision()
        .unwrap()
        .is_none());
    assert_eq!(retained.receiver().unwrap().staged_publication_bytes(), 0);
    if lose_cache {
        retained.lose_resource_cache().unwrap();
    }
    let mut c = retained
        .reconnect(
            server.address,
            "localhost".into(),
            include_bytes!("data/localhost.der").to_vec(),
            credential,
            hello(id),
        )
        .await
        .unwrap();
    assert_eq!(c.view_id, view);
    c.open_view().await.unwrap();
    assert!(matches!(event(&mut c).await, ClientEvent::ViewOpened(_)));
    assert!(matches!(
        event(&mut c).await,
        ClientEvent::ResumeAccepted(_)
    ));
    applied(&mut c, revision).await;
    assert!(c.receiver().presented_revision().unwrap().is_none());
    c
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn sustained_surfaces_cache_eviction_reconnect_and_client_retention_remain_bounded() {
    let mut pairing = PairingAuthority::new(8).unwrap();
    let mut credentials = Vec::new();
    for id in 3..=7 {
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
            ledger: rui_protocol_core::commands::LedgerLimits {
                outcome_ttl_ms: 600_000,
                ..Default::default()
            },
            session: SessionConfig {
                max_clients: 4,
                max_history_records: 1,
                ..Default::default()
            },
            ..Default::default()
        },
        SECRET.into(),
        Arc::new(SyntheticPublicShaper),
    )
    .unwrap();
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
            max_message_bytes: 1024,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut c = connect(&server, &credentials[0], 3).await;
    let mut o = connect(&server, &credentials[1], 4).await;
    c.open_view().await.unwrap();
    o.open_view().await.unwrap();
    tokio::join!(applied(&mut c, 1), applied(&mut o, 1));
    c.presented(1).await.unwrap();
    c.acquire_control([9; 16]).await.unwrap();
    let ClientEvent::ControlGranted(mut grant) = event(&mut c).await else {
        panic!()
    };
    let baseline_records = c.receiver().state().unwrap().records.len();
    let mut revision = 1;
    let mut sequence = 0;
    let mut first_command = None;
    let mut first_result = None;
    let mut evictions = 0;
    let mut prior_retired = 0;
    let mut peak_host = 0;
    let mut peak_client = 0;
    for cycle in 0..128u64 {
        if cycle > 0 {
            let mut request = [6; 16];
            request[..8].copy_from_slice(&cycle.to_be_bytes());
            c.send(
                p::Kind::RenewControl,
                &p::RenewControl {
                    request_id: request.to_vec(),
                    lease_id: grant.lease_id.clone(),
                    generation: grant.generation,
                },
            )
            .await
            .unwrap();
            let ClientEvent::ControlGranted(renewed) = event(&mut c).await else {
                panic!()
            };
            grant = renewed;
        }
        for target in [
            rui_host_broker::COUNTER_ID,
            POPUP_TOGGLE_ID,
            POPUP_TOGGLE_ID,
        ] {
            sequence += 1;
            revision += 1;
            let mut command =
                core::SemanticCommand::decode(activate(&c, &grant, sequence).as_slice()).unwrap();
            command.target_id = target;
            let Record::Semantic(node) = &c.receiver().state().unwrap().records[&target] else {
                panic!()
            };
            command.preconditions.as_mut().unwrap().target_version = Some(node.version);
            let bytes = command.encode_to_vec();
            c.submit(&bytes).await.unwrap();
            let (result, _) = tokio::join!(
                effect(&mut c, revision, sequence),
                applied(&mut o, revision)
            );
            if first_command.is_none() {
                first_command = Some(bytes);
                first_result = Some(result);
            }
            c.presented(revision).await.unwrap();
        }
        for peer in [&mut c, &mut o] {
            for _ in 0..512 {
                if peer.evict_unpinned_resource().await.unwrap().is_none() {
                    break;
                }
                evictions += 1;
            }
            pong(peer, cycle + 1).await;
        }
        {
            let host = server.host.lock().await;
            for (id, peer) in [(3, &c), (4, &o)] {
                let capture = host
                    .source_capture(&authenticated(id), peer.view_id)
                    .unwrap();
                assert_eq!(
                    capture.state().records,
                    peer.receiver().state().unwrap().records
                );
                assert_eq!(capture.state().records.len(), baseline_records);
                assert_eq!(
                    peer.receiver().cache_bytes().unwrap(),
                    capture.resources().map(|(_, b)| b.len()).sum::<usize>()
                );
                assert_eq!(peer.receiver().staged_publication_bytes(), 0);
                assert!(
                    host.known_resource_count(&authenticated(id), peer.view_id)
                        .unwrap()
                        <= 64
                );
                let memory = host
                    .retained_memory_bytes(&authenticated(id), peer.view_id)
                    .unwrap();
                assert!(memory <= peer.limits.view_memory_bytes);
                peak_host = peak_host.max(memory);
                assert!(peer.receiver().memory_used() <= peer.limits.view_memory_bytes);
                peak_client = peak_client.max(peer.receiver().memory_used());
                assert_eq!(
                    host.queued_bytes(&authenticated(id), peer.view_id).unwrap(),
                    0
                );
            }
        }
        let retired = o.receiver().state().unwrap().retired.len();
        assert!(retired > prior_retired);
        prior_retired = retired;
        if cycle % 8 == 7 {
            o = recover(&server, o, &credentials[1], 4, revision, cycle % 16 == 15).await;
            let old_generation = grant.generation;
            c = recover(&server, c, &credentials[0], 3, revision, false).await;
            // Exact original outcome survives many newer commands, source
            // incarnations and expired leases without a second source effect.
            c.submit(first_command.as_ref().unwrap()).await.unwrap();
            let ClientEvent::CommandResult(retry) = event(&mut c).await else {
                panic!()
            };
            assert_eq!(&retry, first_result.as_ref().unwrap());
            assert_eq!(
                server.host.lock().await.diagnostics().source_commit,
                revision
            );
            c.presented(revision).await.unwrap();
            let mut request = [8; 16];
            request[..8].copy_from_slice(&cycle.to_be_bytes());
            c.acquire_control(request).await.unwrap();
            let ClientEvent::ControlGranted(next) = event(&mut c).await else {
                panic!()
            };
            assert!(next.generation > old_generation);
            grant = next;
        }
    }
    assert!(evictions > 100);
    assert_eq!(server.host.lock().await.diagnostics().counter, 128);
    // Disconnected client identities retain bounded replay metadata. Filling
    // the configured four slots fails closed instead of forgetting sequences.
    for id in [5, 6] {
        let mut extra = connect(&server, &credentials[(id - 3) as usize], id).await;
        extra.open_view().await.unwrap();
        applied(&mut extra, 1).await;
        extra.close().await.unwrap();
    }
    assert!(NetworkClient::connect(
        server.address,
        "localhost".into(),
        include_bytes!("data/localhost.der").to_vec(),
        &credentials[4],
        hello(7)
    )
    .await
    .is_err());
    assert_eq!(server.host.lock().await.diagnostics().clients, 4);
    c.submit(first_command.as_ref().unwrap()).await.unwrap();
    assert!(
        matches!(event(&mut c).await,ClientEvent::CommandResult(r) if Some(&r)==first_result.as_ref())
    );
    assert_eq!(server.host.lock().await.diagnostics().counter, 128);
    let cb = c.receiver().memory_budget();
    let ob = o.receiver().memory_budget();
    c.close().await.unwrap();
    o.close().await.unwrap();
    server.shutdown().await;
    assert_eq!(cb.used(), 0);
    assert_eq!(ob.used(), 0);
    eprintln!("sustained: 128 surface incarnations, 384 effects, 32 reconnects, {evictions} retired resource evictions, {prior_retired} retired IDs, sampled host {peak_host} and receiver {peak_client} bytes");
}
