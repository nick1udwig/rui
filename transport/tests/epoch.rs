//! Actual loss of the dispatch ledger with TLS identity and approval surviving.
#[path = "common/resources.rs"]
#[allow(dead_code)]
mod fixture;
use fixture::*;
use rui_host_broker::{ControlledHost, HostConfig};
use rui_transport::{ClientEvent, HostIdentity, ServerConfig, ServerHandle};
use std::{sync::Arc, time::Duration};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broker_restart_changes_epoch_and_never_repeats_unconfirmed_old_commands() {
    let (server, credential, _, pairing) = setup().await;
    let mut client = connect(&server, &credential, 3).await;
    client.open_view().await.unwrap();
    applied(&mut client, 1).await;
    // Explicit simulated renderer callback; native presentation is a separate gate.
    client.presented(1).await.unwrap();
    client.acquire_control([11; 16]).await.unwrap();
    let ClientEvent::ControlGranted(grant) = client.receive().await.unwrap() else {
        panic!("expected initial control");
    };
    let ambiguous = activate(&client, &grant, 1);
    let never_sent = activate(&client, &grant, 2);
    client.submit(&ambiguous).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while server.host.lock().await.diagnostics().counter != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // No result/publication is read after dispatch. The client cannot know
    // whether the action took effect when this broker and its ledger disappear.
    let old_epoch = client.epoch;
    let old_budget = client.receiver().memory_budget();
    let mut retained = client.into_retained();
    server.shutdown().await;

    let config = HostConfig::fresh().unwrap();
    assert_ne!(config.epoch, old_epoch);
    let another = HostConfig::fresh().unwrap();
    assert_ne!(config.epoch, another.epoch);
    assert_ne!(config.session_id, another.session_id);
    let host = ControlledHost::new_corpus_with_fonts(
        config,
        SECRET.into(),
        Arc::new(SyntheticPublicShaper),
    )
    .unwrap();
    let server = ServerHandle::bind(
        "127.0.0.1:0".parse().unwrap(),
        HostIdentity::from_der(
            include_bytes!("data/localhost.der").to_vec(),
            include_bytes!("data/localhost-key.der").to_vec(),
        )
        .unwrap(),
        host,
        pairing,
        ServerConfig::default(),
    )
    .await
    .unwrap();

    for (index, old_command) in [&ambiguous, &never_sent].into_iter().enumerate() {
        let mut client = retained
            .reconnect(
                server.address,
                "localhost".into(),
                include_bytes!("data/localhost.der").to_vec(),
                &credential,
                hello(3),
            )
            .await
            .unwrap();
        assert_ne!(client.epoch, old_epoch);
        assert_eq!(old_budget.used(), 0);
        client.open_view().await.unwrap();
        let mut reset = false;
        loop {
            match client.receive().await.unwrap() {
                ClientEvent::ResetRequired(_) => reset = true,
                ClientEvent::ViewOpened(_)
                | ClientEvent::ResumeAccepted(_)
                | ClientEvent::ResourcesNeeded { .. }
                | ClientEvent::ResourceAvailable => {}
                ClientEvent::Applied { revision } => {
                    assert_eq!(revision, 1);
                    client.flush_pending().await.unwrap();
                    break;
                }
                event => panic!("unexpected restart event: {event:?}"),
            }
        }
        if old_command == &ambiguous {
            assert!(reset, "old epoch must require a fresh checkpoint");
        }
        assert!(client.receiver().presented_revision().unwrap().is_none());
        // Even fresh readiness and a new lease cannot authorize old-epoch bytes.
        client.presented(1).await.unwrap();
        client
            .acquire_control([12 + index as u8; 16])
            .await
            .unwrap();
        loop {
            match client.receive().await.unwrap() {
                ClientEvent::ControlGranted(_) => break,
                ClientEvent::ControlRevoked(_) => {}
                event => panic!("unexpected replacement grant: {event:?}"),
            }
        }
        client.submit(old_command).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(3), client.receive())
                .await
                .unwrap()
                .is_err()
        );
        let state = server.host.lock().await.diagnostics();
        assert_eq!(state.counter, 0);
        assert_eq!(state.source_commit, 1);
        assert_eq!(state.consumed_commands, 0);
        retained = client.into_retained();
    }

    let mut client = retained
        .reconnect(
            server.address,
            "localhost".into(),
            include_bytes!("data/localhost.der").to_vec(),
            &credential,
            hello(3),
        )
        .await
        .unwrap();
    client.open_view().await.unwrap();
    loop {
        if matches!(
            client.receive().await.unwrap(),
            ClientEvent::Applied { revision: 1 }
        ) {
            client.flush_pending().await.unwrap();
            break;
        }
    }
    client.presented(1).await.unwrap();
    client.acquire_control([14; 16]).await.unwrap();
    let ClientEvent::ControlGranted(grant) = client.receive().await.unwrap() else {
        panic!("expected replacement control");
    };
    // The new epoch has a fresh producer sequence, independent of the old ledger.
    client.submit(&activate(&client, &grant, 1)).await.unwrap();
    let mut outcome = false;
    let mut publication = false;
    while !outcome || !publication {
        match client.receive().await.unwrap() {
            ClientEvent::CommandResult(result) => {
                assert_eq!(result.command_seq, 1);
                assert_eq!(result.status, "Applied");
                outcome = true;
            }
            ClientEvent::Applied { revision: 2 } => {
                client.flush_pending().await.unwrap();
                publication = true;
            }
            ClientEvent::ResourcesNeeded { .. } | ClientEvent::ResourceAvailable => {}
            event => panic!("unexpected replacement effect: {event:?}"),
        }
    }
    assert_eq!(server.host.lock().await.diagnostics().counter, 1);
    assert_eq!(server.host.lock().await.diagnostics().consumed_commands, 1);
    let budget = client.receiver().memory_budget();
    client.close().await.unwrap();
    assert_eq!(budget.used(), 0);
    server.shutdown().await;
}
