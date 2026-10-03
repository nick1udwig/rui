//! Compound producer transactions independently of a socket. Native adapters
//! use this same boundary; these tests exercise the controlled reference owner.
#[path = "common/resources.rs"]
#[allow(dead_code)]
mod fixture;
use prost::Message;
use rui_host_broker::{AuthorizationLifetime, ControlledHost, HostConfig, COUNTER_ID};
use rui_protocol_core::{
    commands::{AuthenticatedClient, CommandFamily},
    session::{
        ClientHello, ControlLease, Permissions, ResumeDisposition, ResumeRequest, SessionConfig,
        SharedPayload, VerifiedAuthorization,
    },
    wire::proto as p,
};
use rui_transport::{producer::*, TransportError};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::Mutex;

fn auth() -> VerifiedAuthorization {
    VerifiedAuthorization {
        principal: "source-controller".into(),
        policy_version: 1,
        permissions: Permissions {
            can_view: true,
            can_control: true,
            actions: ["Activate".into()].into_iter().collect(),
        },
    }
}
fn hello(id: u8) -> ClientHello {
    ClientHello {
        client_instance_id: [id; 16],
        supported_versions: [1].into_iter().collect(),
        supported_profiles: SessionConfig::default().supported_profiles,
        required_profiles: SessionConfig::default().required_profiles,
        receiver_limits: Default::default(),
        resume: None,
    }
}
fn setup() -> ControlledPort {
    ControlledPort::new(
        Arc::new(Mutex::new(
            ControlledHost::new(HostConfig::default(), String::new()).unwrap(),
        )),
        Arc::new(|_| {}),
        4,
    )
    .unwrap()
}
async fn connect(
    port: &ControlledPort,
    hello: ClientHello,
    lifetime: AuthorizationLifetime,
) -> Result<Connection, TransportError> {
    let received = port
        .transact(Request::Connect {
            authorization: auth(),
            hello: Box::new(hello),
            lifetime,
            fresh_view: false,
            now: 0,
        })
        .await?;
    let Reply::Connected(connection) = received.value() else {
        panic!("connect reply")
    };
    Ok(connection.clone())
}
async fn call(
    port: &ControlledPort,
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
async fn open(port: &ControlledPort, connection: &Connection) -> SharedPayload {
    let reply = call(
        port,
        connection,
        Operation::Open {
            selection: "fixture".into(),
            mirror_canvas_policy: "host-canvas".into(),
        },
    )
    .await
    .unwrap();
    let Reply::Opened {
        checkpoint: Some(frame),
        ..
    } = reply.value()
    else {
        panic!("fresh checkpoint")
    };
    frame.clone()
}
async fn ready(port: &ControlledPort, connection: &Connection, frame: &SharedPayload) {
    call(
        port,
        connection,
        Operation::Ack(Acknowledgement::Applied {
            revision: 1,
            witness: frame.clone(),
        }),
    )
    .await
    .unwrap();
    call(
        port,
        connection,
        Operation::Ack(Acknowledgement::Presented { revision: 1 }),
    )
    .await
    .unwrap();
}
fn command(connection: &Connection, lease: &ControlLease, seq: u64) -> Vec<u8> {
    p::SemanticCommand {
        epoch: connection.session().epoch.to_vec(),
        view_id: connection.session().view_id.to_vec(),
        client_instance_id: connection.session().client.client_instance_id.to_vec(),
        command_seq: seq,
        control_lease_id: lease.lease_id.to_vec(),
        control_generation: lease.generation,
        target_id: COUNTER_ID,
        action_id: "Activate".into(),
        action_schema_version: 1,
        observed_view_revision: 1,
        preconditions: Some(p::Preconditions {
            target_version: Some(1),
            ..Default::default()
        }),
        arguments: Some(p::semantic_command::Arguments::NoArguments(
            p::EmptyArguments {},
        )),
    }
    .encode_to_vec()
}
#[tokio::test]
async fn same_owner_dispatches_exact_retry_once_and_binds_incarnation() {
    let port = setup();
    let connection = connect(&port, hello(3), AuthorizationLifetime::new())
        .await
        .unwrap();
    let frame = open(&port, &connection).await;
    ready(&port, &connection, &frame).await;
    let grant = call(
        &port,
        &connection,
        Operation::Control(Control::Acquire { request: [9; 16] }),
    )
    .await
    .unwrap();
    let Reply::Lease(lease) = grant.value() else {
        panic!()
    };
    let bytes = command(&connection, lease, 1);
    let first = call(
        &port,
        &connection,
        Operation::Command {
            family: CommandFamily::Semantic,
            bytes: bytes.clone().into(),
        },
    )
    .await
    .unwrap();
    let retry = call(
        &port,
        &connection,
        Operation::Command {
            family: CommandFamily::Semantic,
            bytes: bytes.into(),
        },
    )
    .await
    .unwrap();
    let (Reply::Command(a), Reply::Command(b)) = (first.value(), retry.value()) else {
        panic!()
    };
    assert_eq!(a, b);
    assert_eq!(port.host().lock().await.diagnostics().counter, 1);
    assert_eq!(port.host().lock().await.diagnostics().consumed_commands, 1);
    port.disconnect(&connection);
    port.flush_lifecycle(0).await.unwrap();
    let replacement = connect(&port, hello(3), AuthorizationLifetime::new())
        .await
        .unwrap();
    assert_eq!(replacement.session().view_id, connection.session().view_id);
    assert!(matches!(
        call(&port, &connection, Operation::Authorize).await,
        Err(TransportError::Authority)
    ));
    port.disconnect(&connection);
    drop(connection);
    port.flush_lifecycle(0).await.unwrap();
    call(&port, &replacement, Operation::Authorize)
        .await
        .unwrap();
    let other = setup();
    assert!(matches!(
        call(&other, &replacement, Operation::Authorize).await,
        Err(TransportError::Authority)
    ));
}
#[tokio::test]
async fn resume_keeper_is_exact_atomic_and_control_is_not_restored() {
    let port = setup();
    let connection = connect(&port, hello(3), AuthorizationLifetime::new())
        .await
        .unwrap();
    let frame = open(&port, &connection).await;
    ready(&port, &connection, &frame).await;
    call(
        &port,
        &connection,
        Operation::Control(Control::Acquire { request: [9; 16] }),
    )
    .await
    .unwrap();
    port.disconnect(&connection);
    port.flush_lifecycle(0).await.unwrap();
    let mut h = hello(3);
    h.resume = Some(ResumeRequest {
        epoch: connection.session().epoch,
        view_id: connection.session().view_id,
        acknowledged_revision: 1,
    });
    let resumed = connect(&port, h, AuthorizationLifetime::new())
        .await
        .unwrap();
    assert!(matches!(
        resumed.session().resume,
        ResumeDisposition::Accepted {
            from_revision: 1,
            through_revision: 1,
            ..
        }
    ));
    let keeper = resumed.resume_keeper().unwrap().clone();
    assert_eq!(keeper, frame);
    // Advance the actual source after Connect. Open must still use the captured
    // keeper while the new source transaction is delivered separately.
    port.host()
        .lock()
        .await
        .set_counter_enabled(false, 1)
        .unwrap();
    let opened = call(
        &port,
        &resumed,
        Operation::Open {
            selection: "fixture".into(),
            mirror_canvas_policy: "host-canvas".into(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        opened.value(),
        Reply::Opened {
            checkpoint: None,
            ..
        }
    ));
    assert_eq!(resumed.resume_keeper().unwrap(), &keeper);
    assert!(matches!(
        call(
            &port,
            &resumed,
            Operation::Control(Control::Acquire { request: [10; 16] })
        )
        .await,
        Err(TransportError::Authority)
    ));
    let pumped = call(
        &port,
        &resumed,
        Operation::Pump {
            take_publication: true,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        pumped.value(),
        Reply::Pump {
            publication: Some(_),
            ..
        }
    ));
}
#[tokio::test]
async fn cancelled_connect_reply_flushes_exact_registration_and_cleanup() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let host = Arc::new(Mutex::new(
        ControlledHost::new(HostConfig::default(), String::new()).unwrap(),
    ));
    let port = ControlledPort::new(
        host,
        Arc::new(move |_| {
            count.fetch_add(1, Ordering::AcqRel);
        }),
        2,
    )
    .unwrap();
    let reply = port
        .transact(Request::Connect {
            authorization: auth(),
            hello: Box::new(hello(3)),
            lifetime: AuthorizationLifetime::new(),
            fresh_view: false,
            now: 0,
        })
        .await
        .unwrap();
    // Simulate an abandoned successful authentication reply; no caller cloned
    // the Connection. Drop must enqueue exact priority disconnect ownership.
    drop(reply);
    port.flush_lifecycle(0).await.unwrap();
    let current = connect(&port, hello(3), AuthorizationLifetime::new())
        .await
        .unwrap();
    let frame = open(&port, &current).await;
    ready(&port, &current, &frame).await;
    call(
        &port,
        &current,
        Operation::Control(Control::Acquire { request: [9; 16] }),
    )
    .await
    .unwrap();
    drop(current);
    port.flush_lifecycle(0).await.unwrap();
    assert_eq!(calls.load(Ordering::Acquire), 1);
    let next = connect(&port, hello(3), AuthorizationLifetime::new())
        .await
        .unwrap();
    call(&port, &next, Operation::Authorize).await.unwrap();
}
#[tokio::test]
async fn connection_reservation_failure_does_not_leave_registration_connected() {
    let port = setup();
    let previous = connect(&port, hello(3), AuthorizationLifetime::new())
        .await
        .unwrap();
    let session = previous.session();
    let view = session.view_id;
    let client = session.client.clone();
    let _frame = open(&port, &previous).await;
    let used = port
        .host()
        .lock()
        .await
        .retained_memory_bytes(&client, view)
        .unwrap();
    let pressure = port
        .host()
        .lock()
        .await
        .reserve_view_memory(&client, view, session.limits.view_memory_bytes - used)
        .unwrap();
    port.disconnect(&previous);
    port.flush_lifecycle(0).await.unwrap();
    assert!(matches!(
        connect(&port, hello(3), AuthorizationLifetime::new()).await,
        Err(TransportError::Limit)
    ));
    port.flush_lifecycle(0).await.unwrap();
    let client = AuthenticatedClient {
        principal: auth().principal,
        client_instance_id: [3; 16],
    };
    assert!(
        !port
            .host()
            .lock()
            .await
            .view_status(&client, view)
            .unwrap()
            .connected
    );
    drop(pressure);
    let connection = connect(&port, hello(3), AuthorizationLifetime::new())
        .await
        .unwrap();
    call(&port, &connection, Operation::Authorize)
        .await
        .unwrap();
}
#[tokio::test]
async fn keeper_failure_disconnects_then_allows_retry_after_pressure_released() {
    let port = setup();
    let connection = connect(&port, hello(3), AuthorizationLifetime::new())
        .await
        .unwrap();
    let connection_memory = port
        .host()
        .lock()
        .await
        .retained_memory_bytes(&connection.session().client, connection.session().view_id)
        .unwrap();
    let frame = open(&port, &connection).await;
    ready(&port, &connection, &frame).await;
    let session = connection.session();
    let client = session.client.clone();
    let view = session.view_id;
    let used = port
        .host()
        .lock()
        .await
        .retained_memory_bytes(&client, view)
        .unwrap();
    // New connection metadata fits; less than a capture's staging working set
    // remains, so exact zero-delta keeper construction fails after negotiation.
    let pressure = port
        .host()
        .lock()
        .await
        .reserve_view_memory(
            &client,
            view,
            session.limits.view_memory_bytes - used - connection_memory - 1024,
        )
        .unwrap();
    port.disconnect(&connection);
    port.flush_lifecycle(0).await.unwrap();
    let mut h = hello(3);
    h.resume = Some(ResumeRequest {
        epoch: session.epoch,
        view_id: view,
        acknowledged_revision: 1,
    });
    assert!(matches!(
        connect(&port, h.clone(), AuthorizationLifetime::new()).await,
        Err(TransportError::Limit)
    ));
    assert!(
        !port
            .host()
            .lock()
            .await
            .view_status(&client, view)
            .unwrap()
            .connected
    );
    port.flush_lifecycle(0).await.unwrap();
    drop(pressure);
    let resumed = connect(&port, h, AuthorizationLifetime::new())
        .await
        .unwrap();
    assert!(resumed.resume_keeper().is_some());
}
#[tokio::test]
async fn request_saturation_and_cancelled_waiter_leave_priority_cleanup_available() {
    let port = setup();
    let connection = connect(&port, hello(3), AuthorizationLifetime::new())
        .await
        .unwrap();
    let frame = open(&port, &connection).await;
    ready(&port, &connection, &frame).await;
    call(
        &port,
        &connection,
        Operation::Control(Control::Acquire { request: [9; 16] }),
    )
    .await
    .unwrap();
    let locked = port.host().lock().await;
    let mut waiters = Vec::new();
    for _ in 0..32 {
        waiters.push(port.transact(Request::Scoped {
            connection: connection.clone(),
            operation: Operation::Authorize,
            now: 0,
        }));
    }
    assert!(matches!(
        call(&port, &connection, Operation::Authorize).await,
        Err(TransportError::Limit)
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(5), &mut waiters[0])
            .await
            .is_err()
    );
    port.disconnect(&connection);
    let mut cleanup = port.flush_lifecycle(0);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(5), &mut cleanup)
            .await
            .is_err()
    );
    assert_eq!(port.flush_lifecycle(0).await, Err(TransportError::Limit));
    drop(waiters);
    drop(locked);
    cleanup.await.unwrap();
    let replacement = connect(&port, hello(3), AuthorizationLifetime::new())
        .await
        .unwrap();
    call(&port, &replacement, Operation::Authorize)
        .await
        .unwrap();
    port.shutdown();
    port.flush_lifecycle(0).await.unwrap();
    assert!(matches!(
        call(&port, &replacement, Operation::Authorize).await,
        Err(TransportError::Closed)
    ));
}
#[tokio::test]
async fn resource_witness_scope_eviction_atomicity_and_clones_keep_their_charge() {
    let host = Arc::new(Mutex::new(
        ControlledHost::new_corpus_with_fonts(
            HostConfig::default(),
            "PRIVATE".into(),
            Arc::new(fixture::SyntheticPublicShaper),
        )
        .unwrap(),
    ));
    let port = ControlledPort::new(host.clone(), Arc::new(|_| {}), 4).unwrap();
    let mut h = hello(3);
    h.supported_profiles
        .insert("glyph.outline-single.v1".into());
    let lifetime = AuthorizationLifetime::new();
    let connection = connect(&port, h.clone(), lifetime.clone()).await.unwrap();
    let frame = open(&port, &connection).await;
    ready(&port, &connection, &frame).await;
    let session = connection.session();
    let capture = host
        .lock()
        .await
        .source_capture(&session.client, session.view_id)
        .unwrap();
    let digest = capture.resources().next().unwrap().0.digest;
    drop(capture);
    let witness = Witnesses {
        in_flight: None,
        applied: Some(frame.clone()),
    };
    assert!(matches!(
        call(
            &port,
            &connection,
            Operation::Resources {
                witnesses: Witnesses::default(),
                digests: vec![digest]
            }
        )
        .await,
        Err(TransportError::Authority)
    ));
    let baseline = host
        .lock()
        .await
        .retained_memory_bytes(&session.client, session.view_id)
        .unwrap();
    let received = call(
        &port,
        &connection,
        Operation::Resources {
            witnesses: witness.clone(),
            digests: vec![digest],
        },
    )
    .await
    .unwrap();
    let Reply::Resources(resources) = received.value() else {
        panic!()
    };
    let resource = resources[0].clone();
    drop(received);
    let held = host
        .lock()
        .await
        .retained_memory_bytes(&session.client, session.view_id)
        .unwrap();
    assert!(held >= baseline + 4096 + 1024);
    assert_eq!(resource.offer().unwrap().digest, digest);
    assert!(!resource.bytes().unwrap().is_empty());
    assert!(matches!(
        call(
            &port,
            &connection,
            Operation::Evicted {
                witnesses: witness.clone(),
                digests: vec![digest, [99; 32]],
                previously_missing: vec![]
            }
        )
        .await,
        Err(TransportError::Authority)
    ));
    // Invalid mixed batch must not clear actual presentation/readiness.
    call(
        &port,
        &connection,
        Operation::Control(Control::Acquire { request: [9; 16] }),
    )
    .await
    .unwrap();
    let received = call(
        &port,
        &connection,
        Operation::Evicted {
            witnesses: witness.clone(),
            digests: vec![digest],
            previously_missing: vec![],
        },
    )
    .await
    .unwrap();
    assert!(matches!(received.value(),Reply::Evicted{missing} if missing==&[digest]));
    drop(received);
    assert!(matches!(
        call(
            &port,
            &connection,
            Operation::Control(Control::Acquire { request: [10; 16] })
        )
        .await,
        Err(TransportError::Authority)
    ));
    h.client_instance_id = [4; 16];
    let observer = connect(&port, h, AuthorizationLifetime::new())
        .await
        .unwrap();
    open(&port, &observer).await;
    assert!(matches!(
        call(
            &port,
            &observer,
            Operation::Resources {
                witnesses: witness,
                digests: vec![digest]
            }
        )
        .await,
        Err(TransportError::Authority)
    ));
    lifetime.revoke();
    assert!(matches!(
        resource.bytes(),
        Err(TransportError::Authentication)
    ));
    drop(resource);
    assert!(matches!(
        call(&port, &connection, Operation::Authorize).await,
        Err(TransportError::Authentication)
    ));
}
