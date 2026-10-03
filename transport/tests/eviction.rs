//! Ordinary retired-cache pressure over actual TLS, distinct from live loss.
#[path = "common/resources.rs"]
mod fixture;
use fixture::*;
use rui_transport::{proto as p, ClientEvent};

async fn pong(client: &mut rui_transport::NetworkClient, nonce: u64) {
    client
        .send(p::Kind::Ping, &p::Ping { nonce })
        .await
        .unwrap();
    assert!(matches!(client.receive().await.unwrap(), ClientEvent::Pong(n) if n == nonce));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn ordinary_retired_eviction_preserves_readiness_and_never_authorizes_a_body() {
    let (server, credential, observer_credential, _) = setup().await;
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
    controller
        .submit(&activate(&controller, &grant, 1))
        .await
        .unwrap();
    tokio::join!(applied(&mut controller, 2), applied(&mut observer, 2));
    observer.presented(2).await.unwrap();
    pong(&mut observer, 1).await;
    let before = observer.receiver().cache_bytes().unwrap();
    let budget = observer.receiver().memory_budget();
    let blocker = budget.reserve(budget.limit() - budget.used()).unwrap();
    assert_eq!(
        observer.evict_unpinned_resource().await,
        Err(rui_transport::TransportError::Limit)
    );
    assert_eq!(observer.receiver().cache_bytes().unwrap(), before);
    assert_eq!(observer.receiver().presented_revision().unwrap(), Some(2));
    drop(blocker);
    let digest = observer.evict_unpinned_resource().await.unwrap().unwrap();
    assert!(observer.receiver().cache_bytes().unwrap() < before);
    assert_eq!(observer.receiver().presented_revision().unwrap(), Some(2));
    assert!(observer.receiver().resources_ready().unwrap());
    pong(&mut observer, 2).await;
    {
        let host = server.host.lock().await;
        assert!(host
            .authorize_resource_eviction(&authenticated(4), observer.view_id, digest)
            .is_err());
        assert!(host
            .resource(&authenticated(4), observer.view_id, digest)
            .is_err());
    }
    observer
        .send(
            p::Kind::NeedResources,
            &p::ResourceIds {
                digests: vec![digest.to_vec()],
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        observer.receive().await.unwrap(),
        ClientEvent::Error(_)
    ));
    // No offer/body event, no readiness loss or hidden publication.
    pong(&mut observer, 3).await;
    assert_eq!(observer.receiver().state().unwrap().revision, 2);
    assert!(observer.receiver().resources_ready().unwrap());
    assert_eq!(observer.evict_unpinned_resource().await.unwrap(), None);
    controller.close().await.unwrap();
    observer.close().await.unwrap();
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn bounded_retired_cache_churn_converges_and_survives_zero_delta_reconnect() {
    let (server, credential, observer_credential, _) = setup().await;
    let mut controller = connect(&server, &credential, 3).await;
    let mut observer = connect(&server, &observer_credential, 4).await;
    controller.open_view().await.unwrap();
    observer.open_view().await.unwrap();
    tokio::join!(applied(&mut controller, 1), applied(&mut observer, 1));
    let initial = server
        .host
        .lock()
        .await
        .known_resource_count(&authenticated(4), observer.view_id)
        .unwrap();
    controller.presented(1).await.unwrap();
    controller.acquire_control([9; 16]).await.unwrap();
    let ClientEvent::ControlGranted(grant) = controller.receive().await.unwrap() else {
        panic!()
    };
    let mut evicted = 0;
    for sequence in 1..=14 {
        let revision = sequence + 1;
        controller
            .submit(&activate(&controller, &grant, sequence))
            .await
            .unwrap();
        tokio::join!(
            applied(&mut controller, revision),
            applied(&mut observer, revision)
        );
        controller.presented(revision).await.unwrap();
        for _ in 0..512 {
            if observer.evict_unpinned_resource().await.unwrap().is_none() {
                break;
            }
            evicted += 1;
        }
        pong(&mut observer, sequence).await;
        let host = server.host.lock().await;
        let capture = host
            .source_capture(&authenticated(4), observer.view_id)
            .unwrap();
        assert_eq!(
            capture.state().records,
            observer.receiver().state().unwrap().records
        );
        assert_eq!(
            observer.receiver().cache_bytes().unwrap(),
            capture
                .resources()
                .map(|(_, body)| body.len())
                .sum::<usize>()
        );
        assert!(
            host.known_resource_count(&authenticated(4), observer.view_id)
                .unwrap()
                <= initial + 2
        );
        assert!(
            host.retained_memory_bytes(&authenticated(4), observer.view_id)
                .unwrap()
                <= observer.limits.view_memory_bytes
        );
        assert!(observer.receiver().memory_used() <= observer.limits.view_memory_bytes);
    }
    assert!(evicted >= 8);
    let view = observer.view_id;
    let mut retained = observer.into_retained();
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
    let mut resumed = false;
    loop {
        match observer.receive().await.unwrap() {
            ClientEvent::ViewOpened(_) => {}
            ClientEvent::ResumeAccepted(r) => {
                assert_eq!(r.from_revision, 15);
                assert_eq!(r.through_revision, 15);
                resumed = true;
            }
            ClientEvent::Applied { revision: 15 } => {
                observer.flush_pending().await.unwrap();
                break;
            }
            other => panic!("unexpected recovery event {other:?}"),
        }
    }
    assert!(resumed);
    assert_eq!(observer.evict_unpinned_resource().await.unwrap(), None);
    assert_eq!(server.host.lock().await.diagnostics().counter, 14);
    controller.close().await.unwrap();
    observer.close().await.unwrap();
    server.shutdown().await;
}
