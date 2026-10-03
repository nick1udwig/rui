#![cfg(feature = "transport")]
//! Actual private EIS owner/port tests. Receipt fixtures here are explicit;
//! actual native mirror presentation remains the separate integrated GUI gate.
#[allow(dead_code)]
mod common;
use common::window;
use prost::Message;
use rui_protocol_core::{
    model::{Rect, SurfaceRole},
    resources::MemoryBudget,
    session::{
        ClientHello, Permissions, ResumeDisposition, ResumeRequest, SessionConfig,
        VerifiedAuthorization,
    },
    wire::proto as pb,
};
use rui_session_shell::{
    actor::{transport::NativeProducerPort, Owner},
    Framebuffer, InjectionScope, RegionGrant,
};
use rui_transport::{producer::*, TransportError};
fn owner(budget: MemoryBudget) -> Owner {
    Owner::spawn(
        RegionGrant::new(
            InjectionScope::IsolatedWholeSession,
            Rect {
                x: 0.,
                y: 0.,
                width: 200.,
                height: 120.,
            },
            Rect {
                x: 40.,
                y: 80.,
                width: 600.,
                height: 240.,
            },
            Framebuffer {
                width: 1000,
                height: 700,
            },
            2.,
        )
        .unwrap(),
        "de",
        vec![window(SurfaceRole::Toplevel, 10., 10., 120., 90., 0)],
        budget,
    )
    .unwrap()
}
fn request(id: u8) -> Request {
    let mut config = SessionConfig::default();
    config.supported_profiles.insert("cursor.vector.v1".into());
    Request::Connect {
        authorization: VerifiedAuthorization {
            principal: format!("native-{id}"),
            policy_version: 1,
            permissions: Permissions {
                can_view: true,
                can_control: true,
                actions: ["RawInputBatch".into()].into_iter().collect(),
            },
        },
        hello: Box::new(ClientHello {
            client_instance_id: [id; 16],
            supported_versions: config.versions,
            supported_profiles: config.supported_profiles,
            required_profiles: config.required_profiles,
            receiver_limits: config.receiver_limits,
            resume: None,
        }),
        lifetime: AuthorizationLifetime::new(),
        fresh_view: false,
        now: 0,
    }
}
async fn connect(port: &NativeProducerPort, id: u8) -> Connection {
    let received = port.transact(request(id)).await.unwrap();
    let Reply::Connected(connection) = received.value() else {
        panic!()
    };
    connection.clone()
}
async fn call(
    port: &NativeProducerPort,
    connection: &Connection,
    operation: Operation,
) -> Result<Received, TransportError> {
    port.transact(Request::Scoped {
        connection: connection.clone(),
        operation,
        now: 0,
    })
    .await
}
async fn ready(port: &NativeProducerPort, connection: &Connection) {
    let received = call(
        port,
        connection,
        Operation::Open {
            selection: "session".into(),
            mirror_canvas_policy: "host-canvas".into(),
        },
    )
    .await
    .unwrap();
    let Reply::Opened {
        description,
        checkpoint: Some(frame),
    } = received.value()
    else {
        panic!()
    };
    assert_eq!(description.input_devices[0].input_mode, "raw.host.v1");
    assert_eq!(description.input_devices[0].text_owner, "HOST_PHYSICAL");
    let frame = frame.clone();
    drop(received);
    let envelope = pb::Envelope::decode(frame.as_ref()).unwrap();
    let revision = pb::Snapshot::decode(envelope.payload.as_slice())
        .unwrap()
        .revision;
    call(
        port,
        connection,
        Operation::Ack(Acknowledgement::Applied {
            revision,
            witness: frame,
        }),
    )
    .await
    .unwrap();
    call(
        port,
        connection,
        Operation::Ack(Acknowledgement::Presented { revision }),
    )
    .await
    .unwrap();
}
#[tokio::test]
#[ignore = "explicit pinned native EIS port scope/release gate"]
async fn session_selection_is_explicit_and_release_keeps_each_request_identity() {
    let budget = MemoryBudget::new(128 << 20);
    let owner = owner(budget.clone());
    let port = NativeProducerPort::new(owner.port()).unwrap();
    let connection = connect(&port, 3).await;
    for selection in ["fixture", "application:16"] {
        assert!(matches!(
            call(
                &port,
                &connection,
                Operation::Open {
                    selection: selection.into(),
                    mirror_canvas_policy: "host-canvas".into()
                }
            )
            .await,
            Err(TransportError::Authority)
        ));
        assert!(port.projected_capture(&connection).await.is_err());
    }
    ready(&port, &connection).await;
    let mut previous = None;
    for id in 1..=3 {
        let received = call(
            &port,
            &connection,
            Operation::Control(Control::Acquire { request: [id; 16] }),
        )
        .await
        .unwrap();
        let Reply::Lease(lease) = received.value() else {
            panic!()
        };
        let lease = lease.clone();
        drop(received);
        assert_ne!(previous, Some(lease.generation));
        previous = Some(lease.generation);
        let release = Control::Release {
            request: [id + 20; 16],
            lease: lease.lease_id,
            generation: lease.generation,
        };
        call(&port, &connection, Operation::Control(release))
            .await
            .unwrap();
        port.flush_lifecycle(0).await.unwrap();
    }
    port.disconnect(&connection);
    port.flush_lifecycle(0).await.unwrap();
    drop(connection);
    port.shutdown();
    port.flush_lifecycle(0).await.unwrap();
    drop(port);
    drop(owner);
    assert_eq!(budget.used(), 0);
}
#[tokio::test]
#[ignore = "explicit pinned native escaped-reservation lifetime gate"]
async fn escaped_socket_allocation_stays_charged_after_native_owner_disposal() {
    let budget = MemoryBudget::new(128 << 20);
    let owner = owner(budget.clone());
    let port = NativeProducerPort::new(owner.port()).unwrap();
    let connection = connect(&port, 3).await;
    let received = call(
        &port,
        &connection,
        Operation::ReserveMemory { bytes: 1 << 20 },
    )
    .await
    .unwrap();
    let memory = received.into_memory().unwrap();
    port.disconnect(&connection);
    port.flush_lifecycle(0).await.unwrap();
    drop(connection);
    port.shutdown();
    port.flush_lifecycle(0).await.unwrap();
    drop(port);
    drop(owner);
    assert_eq!(budget.used(), 1 << 20);
    let pressure = budget.reserve(budget.limit() - budget.used()).unwrap();
    assert!(budget.reserve(1).is_err());
    drop(pressure);
    drop(memory);
    assert_eq!(budget.used(), 0);
}
#[tokio::test]
#[ignore = "explicit pinned native cancelled-auth owner cleanup gate"]
async fn cancelled_connect_response_is_disconnected_without_future_socket_traffic() {
    let budget = MemoryBudget::new(128 << 20);
    let owner = owner(budget.clone());
    let port = NativeProducerPort::new(owner.port()).unwrap();
    let abandoned = port.transact(request(3));
    drop(abandoned); // already queued, receiver lost before owner response
    port.flush_lifecycle(0).await.unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let connection = loop {
        match port.transact(request(3)).await {
            Ok(received) => {
                let Reply::Connected(c) = received.value() else {
                    panic!()
                };
                break c.clone();
            }
            Err(TransportError::ProducerBusy) => {
                assert!(std::time::Instant::now() < deadline);
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
            Err(error) => panic!("unexpected reconnect error {error:?}"),
        }
    };
    port.disconnect(&connection);
    port.shutdown();
    port.flush_lifecycle(0).await.unwrap();
    drop(connection);
    drop(port);
    drop(owner);
    assert_eq!(budget.used(), 0);
}

#[tokio::test]
#[ignore = "explicit pinned native negotiated receiver ceilings gate"]
async fn low_record_depth_and_publication_limits_cannot_install_a_checkpoint() {
    for ceiling in 0..4 {
        let budget = MemoryBudget::new(128 << 20);
        let owner = owner(budget.clone());
        let port = NativeProducerPort::new(owner.port()).unwrap();
        let mut request = request(3);
        let Request::Connect { hello, .. } = &mut request else {
            panic!()
        };
        match ceiling {
            0 => hello.receiver_limits.records = 1,
            1 => hello.receiver_limits.ownership_depth = 1,
            2 => hello.receiver_limits.envelope_bytes = 128,
            3 => hello.receiver_limits.snapshot_bytes = 64,
            _ => unreachable!(),
        }
        let received = port.transact(request).await.unwrap();
        let Reply::Connected(connection) = received.value() else {
            panic!()
        };
        let connection = connection.clone();
        drop(received);
        assert!(
            call(
                &port,
                &connection,
                Operation::Open {
                    selection: "session".into(),
                    mirror_canvas_policy: "host-canvas".into(),
                }
            )
            .await
            .is_err(),
            "ceiling {ceiling}"
        );
        assert!(port.projected_capture(&connection).await.is_err());
        assert!(call(
            &port,
            &connection,
            Operation::Ack(Acknowledgement::Presented { revision: 1 })
        )
        .await
        .is_err());
        assert!(call(
            &port,
            &connection,
            Operation::Control(Control::Acquire { request: [1; 16] })
        )
        .await
        .is_err());
        port.disconnect(&connection);
        port.shutdown();
        port.flush_lifecycle(0).await.unwrap();
        drop(connection);
        drop(port);
        drop(owner);
        assert_eq!(budget.used(), 0);
    }
}

#[tokio::test]
#[ignore = "explicit pinned native offline principal and retained ceiling scope gate"]
async fn spoofed_fresh_request_preserves_offline_history_and_changed_limits_mint_a_view() {
    for ceiling in 0..3 {
        let budget = MemoryBudget::new(128 << 20);
        let owner = owner(budget.clone());
        let port = NativeProducerPort::new(owner.port()).unwrap();
        let connection = connect(&port, 3).await;
        ready(&port, &connection).await;
        let epoch = connection.session().epoch;
        let view = connection.session().view_id;
        port.disconnect(&connection);
        port.flush_lifecycle(0).await.unwrap();
        let mut spoof = request(3);
        let Request::Connect {
            authorization,
            fresh_view,
            ..
        } = &mut spoof
        else {
            panic!()
        };
        authorization.principal = "different-authenticated-principal".into();
        *fresh_view = true;
        assert!(matches!(
            port.transact(spoof).await,
            Err(TransportError::Authentication)
        ));
        let mut retry = request(3);
        let Request::Connect { hello, .. } = &mut retry else {
            panic!()
        };
        hello.resume = Some(ResumeRequest {
            epoch,
            view_id: view,
            acknowledged_revision: 1,
        });
        let received = port.transact(retry).await.unwrap();
        let Reply::Connected(resumed) = received.value() else {
            panic!()
        };
        assert_eq!(resumed.session().view_id, view);
        assert!(matches!(
            resumed.session().resume,
            ResumeDisposition::Accepted {
                from_revision: 1,
                through_revision: 1,
                ..
            }
        ));
        assert!(resumed.resume_keeper().is_some());
        let resumed = resumed.clone();
        drop(received);
        port.disconnect(&resumed);
        port.flush_lifecycle(0).await.unwrap();
        let mut reduced = request(3);
        let Request::Connect { hello, .. } = &mut reduced else {
            panic!()
        };
        hello.resume = Some(ResumeRequest {
            epoch,
            view_id: view,
            acknowledged_revision: 1,
        });
        match ceiling {
            0 => hello.receiver_limits.records = 1,
            1 => hello.receiver_limits.ownership_depth = 1,
            2 => hello.receiver_limits.envelope_bytes = 128,
            _ => unreachable!(),
        }
        let received = port.transact(reduced).await.unwrap();
        let Reply::Connected(fresh) = received.value() else {
            panic!()
        };
        assert_ne!(fresh.session().view_id, view);
        assert!(matches!(
            fresh.session().resume,
            ResumeDisposition::ResetRequired(_)
        ));
        assert!(fresh.resume_keeper().is_none());
        let fresh = fresh.clone();
        drop(received);
        assert!(call(
            &port,
            &fresh,
            Operation::Open {
                selection: "session".into(),
                mirror_canvas_policy: "host-canvas".into()
            }
        )
        .await
        .is_err());
        assert!(port.projected_capture(&fresh).await.is_err());
        assert!(call(&port, &connection, Operation::Authorize)
            .await
            .is_err());
        port.disconnect(&fresh);
        port.shutdown();
        port.flush_lifecycle(0).await.unwrap();
        drop(fresh);
        drop(resumed);
        drop(connection);
        drop(port);
        drop(owner);
        assert_eq!(budget.used(), 0);
    }
}

#[tokio::test]
#[ignore = "explicit pinned native trusted diagnostic scope/approval gate"]
async fn named_diagnostic_capture_requires_current_approved_scope_and_retains_its_charge() {
    let budget = MemoryBudget::new(128 << 20);
    let owner = owner(budget.clone());
    let port = NativeProducerPort::new(owner.port()).unwrap();
    let request = request(3);
    let Request::Connect { lifetime, .. } = &request else {
        panic!()
    };
    let approval = lifetime.clone();
    let received = port.transact(request).await.unwrap();
    let Reply::Connected(connection) = received.value() else {
        panic!()
    };
    let connection = connection.clone();
    drop(received);
    let client = connection.session().client.client_instance_id;
    let view = connection.session().view_id;
    assert!(port.presented_revision(client, view).await.is_err());
    let opened = call(
        &port,
        &connection,
        Operation::Open {
            selection: "session".into(),
            mirror_canvas_policy: "host-canvas".into(),
        },
    )
    .await
    .unwrap();
    let Reply::Opened {
        checkpoint: Some(frame),
        ..
    } = opened.value()
    else {
        panic!()
    };
    let frame = frame.clone();
    drop(opened);
    let envelope = pb::Envelope::decode(frame.as_ref()).unwrap();
    let revision = pb::Snapshot::decode(envelope.payload.as_slice())
        .unwrap()
        .revision;
    let used = budget.used();
    for _ in 0..16 {
        assert_eq!(port.presented_revision(client, view).await.unwrap(), None);
        assert_eq!(budget.used(), used, "read retained a new ownership charge");
    }
    call(
        &port,
        &connection,
        Operation::Ack(Acknowledgement::Applied {
            revision,
            witness: frame,
        }),
    )
    .await
    .unwrap();
    assert_eq!(port.presented_revision(client, view).await.unwrap(), None);
    call(
        &port,
        &connection,
        Operation::Ack(Acknowledgement::Presented { revision }),
    )
    .await
    .unwrap();
    assert_eq!(
        port.presented_revision(client, view).await.unwrap(),
        Some(revision)
    );
    assert!(port.presented_revision([99; 16], view).await.is_err());
    assert!(port.presented_revision(client, [99; 16]).await.is_err());
    let capture = port.projected_source(client, view).await.unwrap();
    assert_eq!(
        capture.state(),
        port.projected_capture(&connection).await.unwrap().state()
    );
    assert!(port.projected_source([99; 16], view).await.is_err());
    assert!(port.projected_source(client, [99; 16]).await.is_err());
    call(&port, &connection, Operation::ResourcesMissing)
        .await
        .unwrap();
    assert_eq!(port.presented_revision(client, view).await.unwrap(), None);
    approval.revoke();
    assert!(port.presented_revision(client, view).await.is_err());
    assert!(port.projected_source(client, view).await.is_err());
    port.shutdown();
    port.flush_lifecycle(0).await.unwrap();
    drop(connection);
    drop(port);
    drop(owner);
    assert!(budget.used() > 0);
    assert!(budget.reserve(budget.limit() - budget.used() + 1).is_err());
    drop(capture);
    assert_eq!(budget.used(), 0);
}

#[tokio::test]
#[ignore = "explicit pinned native retained-reducer checkpoint gate"]
async fn unchanged_source_reset_advances_publication_without_inventing_source_effects() {
    let budget = MemoryBudget::new(128 << 20);
    let owner = owner(budget.clone());
    let port = NativeProducerPort::new(owner.port()).unwrap();
    let first = connect(&port, 3).await;
    ready(&port, &first).await;
    let before = port.projected_capture(&first).await.unwrap();
    let view = first.session().view_id;
    port.disconnect(&first);
    port.flush_lifecycle(0).await.unwrap();
    let second = connect(&port, 3).await;
    assert_eq!(second.session().view_id, view);
    assert!(matches!(
        second.session().resume,
        ResumeDisposition::FreshSnapshot
    ));
    let received = call(
        &port,
        &second,
        Operation::Open {
            selection: "session".into(),
            mirror_canvas_policy: "host-canvas".into(),
        },
    )
    .await
    .unwrap();
    let Reply::Opened {
        checkpoint: Some(bytes),
        ..
    } = received.value()
    else {
        panic!()
    };
    let envelope = pb::Envelope::decode(bytes.as_ref()).unwrap();
    let snapshot = pb::Snapshot::decode(envelope.payload.as_slice()).unwrap();
    let after = port.projected_capture(&second).await.unwrap();
    assert_eq!(snapshot.revision, before.state().revision + 1);
    assert_eq!(after.state().revision, snapshot.revision);
    assert_eq!(after.source_revision(), before.source_revision());
    assert_eq!(snapshot.records, before.records());
    assert_eq!(after.state().records, before.state().records);
    assert_eq!(after.state().retired, before.state().retired);
    port.disconnect(&second);
    port.flush_lifecycle(0).await.unwrap();
    drop(received);
    drop(after);
    drop(before);
    drop(first);
    drop(second);
    drop(port);
    drop(owner);
    assert_eq!(budget.used(), 0);
}
