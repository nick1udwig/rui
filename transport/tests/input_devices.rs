//! Advertised raw devices are scoped to authenticated approved visual views.
#[path = "common/resources.rs"]
#[allow(dead_code)]
mod fixture;
use fixture::*;
use rui_host_broker::{FIXTURE_DEVICE, FIXTURE_SEAT};
use rui_transport::{ClientEvent, NetworkClient, ServerConfig};
#[tokio::test]
async fn raw_grants_follow_policy_and_escaped_metadata_keeps_its_view_charge() {
    let mut approval = auth(3, true);
    approval.permissions.actions.insert("RawInputBatch".into());
    let (server, credential, observer, _) = setup_with_authorization(
        ServerConfig {
            max_message_bytes: 1024,
            ..Default::default()
        },
        approval,
    )
    .await;
    let mut controller = connect(&server, &credential, 3).await;
    controller.open_view().await.unwrap();
    let ClientEvent::ViewOpened(opened) = controller.receive().await.unwrap() else {
        panic!("expected Open metadata")
    };
    assert_eq!(opened.input_devices.len(), 1);
    let grant = &opened.input_devices[0];
    assert_eq!(
        (&*grant.seat_id, &*grant.device_id),
        (FIXTURE_SEAT, FIXTURE_DEVICE)
    );
    assert_eq!(
        (
            &*grant.key_namespace,
            &*grant.input_mode,
            &*grant.text_owner
        ),
        ("linux.evdev", "raw.pointer.v1", "CLIENT_TEXT")
    );
    let budget = controller.receiver().memory_budget();
    drop(controller);
    assert!(
        budget.used() > 0,
        "escaped metadata must retain its reservation"
    );
    drop(opened);
    assert_eq!(budget.used(), 0);
    let mut viewer = connect(&server, &observer, 4).await;
    viewer.open_view().await.unwrap();
    let ClientEvent::ViewOpened(opened) = viewer.receive().await.unwrap() else {
        panic!("expected observer Open metadata")
    };
    assert!(opened.input_devices.is_empty());
    drop(opened);
    drop(viewer);
    let mut h = hello(3);
    h.supported_profiles.insert("semantics.only.v1".into());
    h.required_profiles.insert("semantics.only.v1".into());
    let mut semantic = NetworkClient::connect(
        server.address,
        "localhost".into(),
        include_bytes!("data/localhost.der").to_vec(),
        &credential,
        h,
    )
    .await
    .unwrap();
    semantic.open_view().await.unwrap();
    let ClientEvent::ViewOpened(opened) = semantic.receive().await.unwrap() else {
        panic!("expected semantic Open metadata")
    };
    assert!(opened.input_devices.is_empty());
    drop(opened);
    drop(semantic);
    server.shutdown().await;
}
