//! Actual socket loss/re-authentication with the retained reducer and cache.
#[path = "common/resources.rs"]
mod fixture;
use fixture::*;
use rui_protocol_core::{
    commands::Id,
    model::{Record, TextStatus},
};
use rui_transport::{
    pairing::Credential, ClientEvent, NetworkClient, RetainedClientView, ServerHandle,
};
use std::time::Duration;

async fn applied(client: &mut NetworkClient, revision: u64) -> (usize, usize) {
    let result = fixture::applied(client, revision).await;
    // A flushed local ACK is not proof that the owner processed it. These warm
    // resume cases require a known broker acknowledgement before socket loss.
    // Ping is serialized behind ACK on the same protocol owner lane.
    client
        .send(
            rui_transport::proto::Kind::Ping,
            &rui_transport::proto::Ping { nonce: 991 },
        )
        .await
        .unwrap();
    assert!(matches!(
        client.receive().await.unwrap(),
        ClientEvent::Pong(991)
    ));
    result
}

async fn offline(server: &ServerHandle, id: u8, view: Id) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if !server
                .host
                .lock()
                .await
                .view_status(&authenticated(id), view)
                .unwrap()
                .connected
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
async fn reconnect(
    server: &ServerHandle,
    credential: &Credential,
    retained: &mut RetainedClientView,
    id: u8,
) -> NetworkClient {
    let mut client = retained
        .reconnect(
            server.address,
            "localhost".into(),
            include_bytes!("data/localhost.der").to_vec(),
            credential,
            hello(id),
        )
        .await
        .unwrap();
    client.open_view().await.unwrap();
    client
}
async fn converged(client: &mut NetworkClient, revision: u64) -> (bool, bool, usize) {
    let mut resumed = false;
    let mut reset = false;
    let mut need = 0;
    for _ in 0..1024 {
        match client.receive().await.unwrap() {
            ClientEvent::ViewOpened(_) => {}
            ClientEvent::ResumeAccepted(_) => resumed = true,
            ClientEvent::ResetRequired(_) => reset = true,
            ClientEvent::ResourcesNeeded { revision: r } => {
                assert_eq!(r, revision);
                need += 1;
            }
            ClientEvent::ResourceAvailable => {}
            ClientEvent::Applied { revision: r } => {
                assert_eq!(r, revision);
                client.flush_pending().await.unwrap();
                return (resumed, reset, need);
            }
            other => panic!("unexpected public recovery event {other:?}"),
        }
    }
    panic!("bounded recovery did not converge")
}
async fn assert_projection(server: &ServerHandle, client: &NetworkClient, id: u8) {
    let host = server.host.lock().await;
    let capture = host
        .source_capture(&authenticated(id), client.view_id)
        .unwrap();
    assert_eq!(
        capture.state().records,
        client.receiver().state().unwrap().records
    );
    assert!(client.receiver().memory_used() <= client.limits.view_memory_bytes);
    assert!(
        host.retained_memory_bytes(&authenticated(id), client.view_id)
            .unwrap()
            <= client.limits.view_memory_bytes
    );
    if id == 4 {
        let Record::Text(private) =
            &client.receiver().state().unwrap().records[&rui_host_broker::SECRET_TEXT_ID]
        else {
            panic!()
        };
        assert_eq!(private.status, TextStatus::Redacted);
        assert!(private.content.is_none());
        assert!(!format!("{:?}", capture.state()).contains(SECRET));
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zero_delta_resume_retains_cache_but_requires_fresh_presentation_and_lease() {
    let (server, credential, _, _) = setup().await;
    let mut client = connect(&server, &credential, 3).await;
    client.open_view().await.unwrap();
    applied(&mut client, 1).await;
    client.presented(1).await.unwrap();
    client.acquire_control([9; 16]).await.unwrap();
    let ClientEvent::ControlGranted(old) = client.receive().await.unwrap() else {
        panic!()
    };
    let old_command = activate(&client, &old, 1);
    let view = client.view_id;
    let cache = client.receiver().cache_bytes().unwrap();
    let before = client.receiver().memory_used();
    let mut retained = client.into_retained();
    assert!(retained.receiver().unwrap().memory_used() < before);
    assert_eq!(
        retained.receiver().unwrap().presented_revision().unwrap(),
        None
    );
    offline(&server, 3, view).await;
    let mut client = reconnect(&server, &credential, &mut retained, 3).await;
    assert_eq!(client.view_id, view);
    assert_eq!(converged(&mut client, 1).await, (true, false, 0));
    assert_eq!(client.receiver().cache_bytes().unwrap(), cache);
    client.acquire_control([8; 16]).await.unwrap();
    assert!(matches!(
        client.receive().await.unwrap(),
        ClientEvent::Error(_)
    ));
    client.submit(&old_command).await.unwrap();
    assert!(
        matches!(client.receive().await.unwrap(),ClientEvent::CommandResult(r) if r.status=="Rejected")
    );
    assert_eq!(server.host.lock().await.diagnostics().counter, 0);
    assert_projection(&server, &client, 3).await;
    client.close().await.unwrap();
    server.shutdown().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn source_changes_resume_when_history_exists_and_checkpoint_when_evicted() {
    for changes in [1, 2] {
        let (server, _, credential, _) = setup().await;
        let mut client = connect(&server, &credential, 4).await;
        client.open_view().await.unwrap();
        applied(&mut client, 1).await;
        let view = client.view_id;
        let mut retained = client.into_retained();
        offline(&server, 4, view).await;
        for change in 0..changes {
            server
                .host
                .lock()
                .await
                .set_counter_enabled(change != 0, change + 1)
                .unwrap();
        }
        let mut client = reconnect(&server, &credential, &mut retained, 4).await;
        assert_eq!(client.view_id, view);
        let revision = if changes == 1 { 2 } else { 4 };
        let decision = converged(&mut client, revision).await;
        assert_eq!((decision.0, decision.1), (changes == 1, changes == 2));
        assert_projection(&server, &client, 4).await;
        client.close().await.unwrap();
        server.shutdown().await;
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupted_chunk_replays_exact_effect_without_redispatching_or_old_lease() {
    let (server, credential, _, _) = setup().await;
    let mut h = hello(3);
    h.receiver_limits.chunk_bytes = 64;
    let mut client = NetworkClient::connect(
        server.address,
        "localhost".into(),
        include_bytes!("data/localhost.der").to_vec(),
        &credential,
        h.clone(),
    )
    .await
    .unwrap();
    client.open_view().await.unwrap();
    applied(&mut client, 1).await;
    client.presented(1).await.unwrap();
    client.acquire_control([9; 16]).await.unwrap();
    let ClientEvent::ControlGranted(lease) = client.receive().await.unwrap() else {
        panic!()
    };
    let command = activate(&client, &lease, 1);
    client.submit(&command).await.unwrap();
    assert!(
        matches!(client.receive().await.unwrap(),ClientEvent::CommandResult(r) if r.status=="Applied")
    );
    for _ in 0..64 {
        assert!(
            tokio::time::timeout(Duration::from_millis(1), client.receive())
                .await
                .is_err()
        );
        if client.receiver().staged_publication_bytes() > 0 {
            break;
        }
    }
    assert!(client.receiver().staged_publication_bytes() > 0);
    assert_eq!(client.receiver().state().unwrap().revision, 1);
    let view = client.view_id;
    let mut retained = client.into_retained();
    assert_eq!(retained.receiver().unwrap().staged_publication_bytes(), 0);
    assert!(!retained.receiver().unwrap().has_pending_publication());
    offline(&server, 3, view).await;
    let mut client = retained
        .reconnect(
            server.address,
            "localhost".into(),
            include_bytes!("data/localhost.der").to_vec(),
            &credential,
            h,
        )
        .await
        .unwrap();
    client.open_view().await.unwrap();
    assert_eq!(converged(&mut client, 2).await, (true, false, 0));
    client.submit(&command).await.unwrap();
    assert!(
        matches!(client.receive().await.unwrap(),ClientEvent::CommandResult(r) if r.status=="Applied")
    );
    client.submit(&activate(&client, &lease, 2)).await.unwrap();
    assert!(
        matches!(client.receive().await.unwrap(),ClientEvent::CommandResult(r) if r.status=="Rejected")
    );
    assert_eq!(server.host.lock().await.diagnostics().counter, 1);
    assert_eq!(server.host.lock().await.diagnostics().consumed_commands, 2);
    client.close().await.unwrap();
    server.shutdown().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zero_delta_resume_restores_lost_live_cache_before_applied() {
    let (server, _, credential, _) = setup().await;
    let mut client = connect(&server, &credential, 4).await;
    client.open_view().await.unwrap();
    applied(&mut client, 1).await;
    let view = client.view_id;
    let mut retained = client.into_retained();
    offline(&server, 4, view).await;
    retained.lose_resource_cache().unwrap();
    assert_eq!(retained.receiver().unwrap().cache_bytes().unwrap(), 0);
    let mut client = reconnect(&server, &credential, &mut retained, 4).await;
    assert_eq!(converged(&mut client, 1).await, (true, false, 1));
    assert!(client.receiver().resources_ready().unwrap());
    assert_projection(&server, &client, 4).await;
    client.close().await.unwrap();
    server.shutdown().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn narrower_limits_and_reapproved_grant_confirm_distinct_views_before_disposal() {
    for reapprove in [false, true] {
        let (server, _, credential, pairing) = setup().await;
        let mut client = connect(&server, &credential, 4).await;
        client.open_view().await.unwrap();
        applied(&mut client, 1).await;
        let view = client.view_id;
        let mut retained = client.into_retained();
        offline(&server, 4, view).await;
        let mut replacement = None;
        if reapprove {
            let mut pairing = pairing.lock().unwrap();
            assert!(pairing.revoke_local([4; 16]));
            let request = pairing.request_local_pairing([4; 16]).unwrap();
            replacement = Some(
                pairing
                    .approve_local(request.request_id, auth(4, false))
                    .unwrap(),
            );
        }
        let credential = replacement.as_ref().unwrap_or(&credential);
        let mut h = hello(4);
        if !reapprove {
            h.receiver_limits.envelope_bytes = 512;
        }
        let mut client = retained
            .reconnect(
                server.address,
                "localhost".into(),
                include_bytes!("data/localhost.der").to_vec(),
                credential,
                h,
            )
            .await
            .unwrap();
        client.open_view().await.unwrap();
        assert_ne!(client.view_id, view);
        assert!(retained.receiver().is_none());
        assert_eq!(client.receiver().state().unwrap().revision, 0);
        let decision = converged(&mut client, 1).await;
        assert!(!decision.0);
        assert_eq!(decision.1, reapprove);
        assert_eq!(decision.2, 1);
        assert_projection(&server, &client, 4).await;
        client.close().await.unwrap();
        server.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn immediate_reconnect_busy_wait_is_bounded_cancellable_and_preserves_owner() {
    let (server, _, credential, _) = setup().await;
    let mut client = connect(&server, &credential, 4).await;
    client.open_view().await.unwrap();
    applied(&mut client, 1).await;
    // Prevent the old task's disconnect from retiring its ActiveGuard, without
    // any server-introspection synchronization before the client reconnect.
    let hold = server.host.lock().await;
    let mut retained = client.into_retained();
    let result = tokio::time::timeout(
        Duration::from_millis(40),
        reconnect(&server, &credential, &mut retained, 4),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(retained.receiver().unwrap().state().unwrap().revision, 1);
    drop(hold);
    let mut client = reconnect(&server, &credential, &mut retained, 4).await;
    assert_eq!(converged(&mut client, 1).await, (true, false, 0));
    client.close().await.unwrap();
    server.shutdown().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_writer_reservation_preserves_retained_owner_for_retry() {
    let (server, _, credential, _) = setup().await;
    let mut client = connect(&server, &credential, 4).await;
    client.open_view().await.unwrap();
    applied(&mut client, 1).await;
    let mut retained = client.into_retained();
    let budget = retained.receiver().unwrap().memory_budget();
    let hold = budget.reserve(budget.limit() - budget.used()).unwrap();
    let result = retained
        .reconnect(
            server.address,
            "localhost".into(),
            include_bytes!("data/localhost.der").to_vec(),
            &credential,
            hello(4),
        )
        .await;
    assert!(matches!(result, Err(rui_transport::TransportError::Limit)));
    assert_eq!(retained.receiver().unwrap().state().unwrap().revision, 1);
    assert_eq!(
        retained.receiver().unwrap().presented_revision().unwrap(),
        None
    );
    drop(hold);
    let mut client = reconnect(&server, &credential, &mut retained, 4).await;
    assert_eq!(converged(&mut client, 1).await, (true, false, 0));
    client.close().await.unwrap();
    server.shutdown().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_atomic_resume_keeper_disconnects_negotiated_host_view() {
    let (server, _, credential, _) = setup().await;
    let mut client = connect(&server, &credential, 4).await;
    client.open_view().await.unwrap();
    applied(&mut client, 1).await;
    let view = client.view_id;
    let hold = {
        let host = server.host.lock().await;
        let used = host.retained_memory_bytes(&authenticated(4), view).unwrap();
        host.reserve_view_memory(
            &authenticated(4),
            view,
            client.limits.view_memory_bytes - used,
        )
        .unwrap()
    };
    let mut retained = client.into_retained();
    offline(&server, 4, view).await;
    assert!(retained
        .reconnect(
            server.address,
            "localhost".into(),
            include_bytes!("data/localhost.der").to_vec(),
            &credential,
            hello(4)
        )
        .await
        .is_err());
    assert!(
        !server
            .host
            .lock()
            .await
            .view_status(&authenticated(4), view)
            .unwrap()
            .connected
    );
    assert_eq!(retained.receiver().unwrap().state().unwrap().revision, 1);
    drop(hold);
    let mut client = reconnect(&server, &credential, &mut retained, 4).await;
    assert_eq!(converged(&mut client, 1).await, (true, false, 0));
    client.close().await.unwrap();
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actor_owned_exact_command_survives_lost_result_and_failed_metadata_reservation() {
    let (server, credential, _, _) = setup().await;
    let mut client = connect(&server, &credential, 3).await;
    client.open_view().await.unwrap();
    applied(&mut client, 1).await;
    client.presented(1).await.unwrap();
    client.acquire_control([51; 16]).await.unwrap();
    let ClientEvent::ControlGranted(lease) = client.receive().await.unwrap() else {
        panic!()
    };
    let exact = activate(&client, &lease, 1);
    client.submit(&exact).await.unwrap();
    assert_eq!(client.commands().len(), 1);
    assert_eq!(client.commands().commands().next().unwrap().bytes(), exact);
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.host.lock().await.diagnostics().counter != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Lose the result after the native effect; recovery has no original UI queue owner.
    let view = client.view_id;
    let envelope = client.limits.envelope_bytes;
    let mut retained = client.into_retained();
    offline(&server, 3, view).await;
    let budget = retained.receiver().unwrap().memory_budget();
    let used = budget.used();
    // The socket writer can reserve, but negotiated metadata cannot. A failed
    // post-handshake allocation must not consume the retained graph/command.
    let writer_charge = envelope * 4 + 64 * 1024;
    let hold = budget
        .reserve(budget.limit() - used - writer_charge)
        .unwrap();
    let result = retained
        .reconnect(
            server.address,
            "localhost".into(),
            include_bytes!("data/localhost.der").to_vec(),
            &credential,
            hello(3),
        )
        .await;
    assert!(matches!(result, Err(rui_transport::TransportError::Limit)));
    assert_eq!(retained.receiver().unwrap().state().unwrap().revision, 1);
    assert_eq!(
        retained
            .commands()
            .unwrap()
            .commands()
            .next()
            .unwrap()
            .bytes(),
        exact
    );
    assert_eq!(budget.used(), used + hold.bytes());
    drop(hold);
    let mut client = reconnect(&server, &credential, &mut retained, 3).await;
    assert_eq!(client.commands().len(), 1);
    assert_eq!(client.commands().commands().next().unwrap().bytes(), exact);
    converged(&mut client, 2).await;
    client.retry_command(1).await.unwrap();
    loop {
        if let ClientEvent::CommandResult(result) = client.receive().await.unwrap() {
            assert_eq!(result.command_seq, 1);
            assert_eq!(result.status, "Applied");
            break;
        }
    }
    assert!(client.commands().is_empty());
    assert_eq!(client.commands().next_sequence(), 2);
    assert_eq!(server.host.lock().await.diagnostics().counter, 1);
    assert_eq!(server.host.lock().await.diagnostics().consumed_commands, 1);
    client.close().await.unwrap();
    server.shutdown().await;
}
