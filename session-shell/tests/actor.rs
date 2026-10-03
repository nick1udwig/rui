#![cfg(feature = "native-eis")]
use prost::Message;
use rui_protocol_core::{
    commands::{CommandStatus, Id},
    model::{Affine, Color, Record, Rect, Size, SurfaceRole, TypedValue},
    resources::MemoryBudget,
    session::{ClientHello, Permissions, ReceiverLimits, SessionConfig, VerifiedAuthorization},
    wire::proto as pb,
};
use rui_session_shell::{
    actor::{Connection, HostPort, LocalEvent, Owner, Response},
    topology::WindowSpec,
    Framebuffer, InjectionScope, RegionGrant,
};
use std::time::{Duration, Instant};
fn window() -> WindowSpec {
    WindowSpec {
        role: SurfaceRole::Toplevel,
        parent: None,
        modal_for: None,
        desktop_rect: Rect {
            x: 10.0,
            y: 10.0,
            width: 120.0,
            height: 90.0,
        },
        content_size: Size {
            width: 120.0,
            height: 90.0,
        },
        transform: Affine {
            xx: 1.0,
            yy: 1.0,
            yx: 0.0,
            xy: 0.0,
            tx: 0.0,
            ty: 0.0,
        },
        stacking_order: 0,
        color: Color {
            red: 0.0,
            green: 0.0,
            blue: 1.0,
            alpha: 1.0,
        },
    }
}
fn owner(budget: MemoryBudget) -> Owner {
    Owner::spawn(
        RegionGrant::new(
            InjectionScope::IsolatedWholeSession,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 200.0,
                height: 120.0,
            },
            Rect {
                x: 40.0,
                y: 80.0,
                width: 600.0,
                height: 240.0,
            },
            Framebuffer {
                width: 1000,
                height: 700,
            },
            2.0,
        )
        .unwrap(),
        "de",
        vec![window()],
        budget,
    )
    .unwrap()
}
fn connect(port: &HostPort, id: u8, control: bool) -> Connection {
    let config = SessionConfig::default();
    let mut supported = config.supported_profiles;
    supported.insert("cursor.vector.v1".into());
    let reply = port
        .negotiate(
            &VerifiedAuthorization {
                principal: format!("native-{id}"),
                policy_version: 1,
                permissions: Permissions {
                    can_view: true,
                    can_control: control,
                    actions: ["RawInputBatch".into()].into_iter().collect(),
                },
            },
            &ClientHello {
                client_instance_id: [id; 16],
                supported_versions: [1].into_iter().collect(),
                supported_profiles: supported,
                required_profiles: config.required_profiles,
                receiver_limits: ReceiverLimits::default(),
                resume: None,
            },
        )
        .unwrap()
        .wait(Duration::from_secs(2))
        .unwrap();
    let Response::Connected { connection, .. } = reply.value() else {
        panic!()
    };
    connection.clone()
}
fn ready(port: &HostPort, c: &Connection) -> u64 {
    let reply = port
        .checkpoint(c)
        .unwrap()
        .wait(Duration::from_secs(2))
        .unwrap();
    let Response::Publication(bytes) = reply.value() else {
        panic!()
    };
    let envelope = pb::Envelope::decode(bytes.as_ref()).unwrap();
    let snapshot = pb::Snapshot::decode(envelope.payload.as_slice()).unwrap();
    // Trusted admission-readiness fixture. Actual GTK after-paint is a
    // separate source-window gate; this does not fabricate a mirror proof.
    port.applied(c, snapshot.revision)
        .unwrap()
        .wait(Duration::from_secs(2))
        .unwrap();
    port.presented(c, snapshot.revision)
        .unwrap()
        .wait(Duration::from_secs(2))
        .unwrap();
    snapshot.revision
}
fn lease(port: &HostPort, c: &Connection, id: Id) -> rui_protocol_core::session::ControlLease {
    let reply = port
        .acquire_control(c, id)
        .unwrap()
        .wait(Duration::from_secs(2))
        .unwrap();
    let Response::Lease(lease) = reply.value() else {
        panic!()
    };
    lease.clone()
}
fn raw(
    port: &HostPort,
    c: &Connection,
    l: &rui_protocol_core::session::ControlLease,
    revision: u64,
    seq: u64,
    events: Vec<pb::raw_event::Event>,
) -> Vec<u8> {
    let reply = port
        .source_capture(c)
        .unwrap()
        .wait(Duration::from_secs(2))
        .unwrap();
    let Response::Source(source) = reply.value() else {
        panic!()
    };
    let surface = source
        .state()
        .records
        .values()
        .find_map(|r| {
            if let Record::Surface(s) = r {
                Some(s)
            } else {
                None
            }
        })
        .unwrap();
    pb::RawInputBatch {
        header: Some(pb::CommandHeader {
            epoch: c.epoch().to_vec(),
            view_id: c.view().to_vec(),
            client_instance_id: c.client().client_instance_id.to_vec(),
            command_seq: seq,
            control_lease_id: l.lease_id.to_vec(),
            control_generation: l.generation,
            observed_view_revision: revision,
        }),
        surface_id: surface.id,
        routing_token: surface.routing_token.clone(),
        presented_revision: revision,
        seat_id: rui_session_shell::backend::SEAT.into(),
        device_id: rui_session_shell::backend::DEVICE.into(),
        key_namespace: "linux.evdev".into(),
        events: events
            .into_iter()
            .enumerate()
            .map(|(i, event)| pb::RawEvent {
                timestamp_us: seq * 1000 + i as u64,
                event: Some(event),
            })
            .collect(),
    }
    .encode_to_vec()
}
fn key(code: u32, pressed: bool) -> pb::raw_event::Event {
    pb::raw_event::Event::Key(pb::KeyEvent { code, pressed })
}
fn submit(port: &HostPort, c: &Connection, bytes: &[u8]) -> CommandStatus {
    let reply = port
        .submit_raw(c, bytes)
        .unwrap()
        .wait(Duration::from_secs(2))
        .unwrap();
    let Response::Command(result) = reply.value() else {
        panic!()
    };
    result.status
}
fn text(port: &HostPort, c: &Connection) -> String {
    let reply = port
        .source_capture(c)
        .unwrap()
        .wait(Duration::from_secs(2))
        .unwrap();
    let Response::Source(source) = reply.value() else {
        panic!()
    };
    source
        .state()
        .records
        .values()
        .find_map(|r| {
            if let Record::Semantic(s) = r {
                if let Some(TypedValue::Text(t)) = &s.value {
                    Some(t.clone())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .unwrap()
}
fn wait_text(port: &HostPort, c: &Connection, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while text(port, c) != expected {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
#[ignore = "explicit pinned libei/libeis/XKB owner-thread gate"]
fn one_owner_admits_once_and_local_native_lane_neutralizes_before_source_edit() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<HostPort>();
    let budget = MemoryBudget::new(128 << 20);
    let owner = owner(budget.clone());
    let port = owner.port();
    let local = owner.local_device();
    let c = connect(&port, 1, true);
    let revision = ready(&port, &c);
    let l = lease(&port, &c, [1; 16]);
    let bytes = raw(
        &port,
        &c,
        &l,
        revision,
        1,
        vec![key(21, true), key(21, false)],
    );
    assert_eq!(submit(&port, &c, &bytes), CommandStatus::Dispatched);
    wait_text(&port, &c, "z");
    assert_eq!(submit(&port, &c, &bytes), CommandStatus::Dispatched);
    assert_eq!(text(&port, &c), "z");
    let bytes = raw(&port, &c, &l, revision, 2, vec![key(42, true)]);
    assert_eq!(submit(&port, &c, &bytes), CommandStatus::Dispatched);
    // Native callback lane is trusted here; an actual GDK origin is exercised
    // separately by the private source-window probe, not inferred by this test.
    local
        .native_event(LocalEvent::Key {
            code: 21,
            pressed: true,
        })
        .unwrap();
    local
        .native_event(LocalEvent::Key {
            code: 21,
            pressed: false,
        })
        .unwrap();
    wait_text(&port, &c, "zz");
    let bytes = raw(&port, &c, &l, revision, 3, vec![key(21, true)]);
    assert_eq!(submit(&port, &c, &bytes), CommandStatus::Rejected);
    let revision = ready(&port, &c);
    let l = lease(&port, &c, [2; 16]);
    let bytes = raw(
        &port,
        &c,
        &l,
        revision,
        4,
        vec![key(21, true), key(21, false)],
    );
    assert_eq!(submit(&port, &c, &bytes), CommandStatus::Dispatched);
    wait_text(&port, &c, "zzz");
    owner.shutdown();
    assert!(port.status().is_err());
    drop(local);
    drop(c);
    drop(port);
    assert_eq!(budget.used(), 0);
}
#[test]
#[ignore = "explicit pinned libei/libeis/XKB owner-thread gate"]
fn observer_disconnect_cannot_revoke_other_controller_and_terminal_priority_survives_pressure() {
    let budget = MemoryBudget::new(128 << 20);
    let owner = owner(budget.clone());
    let port = owner.port();
    let controller = connect(&port, 2, true);
    let observer = connect(&port, 3, false);
    let revision = ready(&port, &controller);
    ready(&port, &observer);
    let l = lease(&port, &controller, [3; 16]);
    let bytes = raw(&port, &controller, &l, revision, 1, vec![key(21, true)]);
    assert_eq!(
        submit(&port, &controller, &bytes),
        CommandStatus::Dispatched
    );
    wait_text(&port, &controller, "z");
    port.disconnect(&observer).unwrap();
    let bytes = raw(&port, &controller, &l, revision, 2, vec![key(21, false)]);
    assert_eq!(
        submit(&port, &controller, &bytes),
        CommandStatus::Dispatched
    );
    let mut pending = Vec::new();
    let mut denied = 0;
    for _ in 0..1000 {
        match port.status() {
            Ok(p) => pending.push(p),
            Err(_) => denied += 1,
        }
    }
    assert!(
        denied > 0,
        "actual bounded request channel did not apply pressure"
    );
    let start = Instant::now();
    owner.shutdown();
    assert!(start.elapsed() < Duration::from_secs(3));
    assert!(port.status().is_err());
    drop(pending);
    drop(controller);
    drop(observer);
    drop(port);
    assert_eq!(budget.used(), 0);
}
#[test]
#[ignore = "explicit pinned libei/libeis/XKB owner-thread gate"]
fn source_and_publication_clones_keep_global_charges_after_owner_shutdown() {
    let budget = MemoryBudget::new(128 << 20);
    let owner = owner(budget.clone());
    let port = owner.port();
    let c = connect(&port, 4, false);
    let reply = port
        .checkpoint(&c)
        .unwrap()
        .wait(Duration::from_secs(2))
        .unwrap();
    let Response::Publication(bytes) = reply.value() else {
        panic!()
    };
    let bytes = bytes.clone();
    drop(reply);
    let local = owner.local_device();
    let source = local
        .source_capture()
        .unwrap()
        .wait(Duration::from_secs(2))
        .unwrap();
    owner.shutdown();
    drop(local);
    drop(c);
    drop(port);
    assert!(budget.used() > 0);
    drop(source);
    assert!(budget.used() > 0);
    drop(bytes);
    assert_eq!(budget.used(), 0);
}

#[test]
#[ignore = "explicit pinned libei/libeis/XKB owner-thread gate"]
fn invalid_local_geometry_is_rejected_before_invalidation_and_owner_remains_usable() {
    let budget = MemoryBudget::new(128 << 20);
    let owner = owner(budget.clone());
    let port = owner.port();
    let local = owner.local_device();
    let c = connect(&port, 5, true);
    let revision = ready(&port, &c);
    let l = lease(&port, &c, [6; 16]);
    assert!(local
        .native_event(LocalEvent::Pointer {
            desktop: rui_protocol_core::model::Point { x: -1.0, y: 0.0 },
            press: true
        })
        .is_err());
    let bytes = raw(
        &port,
        &c,
        &l,
        revision,
        1,
        vec![key(21, true), key(21, false)],
    );
    assert_eq!(submit(&port, &c, &bytes), CommandStatus::Dispatched);
    wait_text(&port, &c, "z");
    owner.shutdown();
    drop(local);
    drop(c);
    drop(port);
    assert_eq!(budget.used(), 0);
}

#[test]
#[ignore = "explicit pinned libei/libeis/XKB owner-thread gate"]
fn owner_startup_error_joins_native_thread_and_releases_all_reservations() {
    let budget = MemoryBudget::new(128 << 20);
    let grant = RegionGrant::new(
        InjectionScope::IsolatedWholeSession,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 200.0,
            height: 120.0,
        },
        Rect {
            x: 40.0,
            y: 80.0,
            width: 600.0,
            height: 240.0,
        },
        Framebuffer {
            width: 1000,
            height: 700,
        },
        2.0,
    )
    .unwrap();
    let mut invalid = window();
    invalid.content_size.width = -1.0;
    assert!(Owner::spawn(grant, "de", vec![invalid], budget.clone()).is_err());
    assert_eq!(budget.used(), 0);
}

#[test]
#[ignore = "explicit pinned native producer-only source-edit validation gate"]
fn producer_source_edits_validate_before_mutation_and_preserve_published_identity() {
    use rui_session_shell::actor::SourceEdit;
    let budget = MemoryBudget::new(128 << 20);
    let owner = owner(budget.clone());
    let port = owner.port();
    let connection = connect(&port, 3, false);
    let before = port
        .source_capture(&connection)
        .unwrap()
        .wait(Duration::from_secs(2))
        .unwrap();
    let Response::Source(source) = before.value() else {
        panic!()
    };
    let surface = source
        .state()
        .records
        .values()
        .find_map(|r| match r {
            Record::Surface(s) => Some(s),
            _ => None,
        })
        .unwrap();
    let mut invalid = window();
    invalid.content_size.width = -1.;
    let mut orphan = window();
    orphan.parent = Some(u64::MAX);
    let mut singular = window().transform;
    singular.xx = 0.;
    for change in [
        SourceEdit::Open(invalid),
        SourceEdit::Open(orphan),
        SourceEdit::Close(u64::MAX),
        SourceEdit::Focus(u64::MAX),
        SourceEdit::Map {
            surface: u64::MAX,
            mapped: false,
        },
        SourceEdit::Configure {
            surface: surface.id,
            desktop_rect: window().desktop_rect,
            content_size: window().content_size,
            transform: singular,
            stacking_order: 4,
        },
    ] {
        assert!(port
            .source_edit(change)
            .unwrap()
            .wait(Duration::from_secs(2))
            .is_err());
        let after = port
            .source_capture(&connection)
            .unwrap()
            .wait(Duration::from_secs(2))
            .unwrap();
        let Response::Source(after) = after.value() else {
            panic!()
        };
        assert_eq!(
            source.state(),
            after.state(),
            "failed source edit changed clock, route or graph"
        );
    }
    let mut spec = window();
    spec.desktop_rect.x = 20.;
    port.source_edit(SourceEdit::Configure {
        surface: surface.id,
        desktop_rect: spec.desktop_rect,
        content_size: spec.content_size,
        transform: spec.transform,
        stacking_order: spec.stacking_order,
    })
    .unwrap()
    .wait(Duration::from_secs(2))
    .unwrap();
    let after_received = port
        .source_capture(&connection)
        .unwrap()
        .wait(Duration::from_secs(2))
        .unwrap();
    let Response::Source(after) = after_received.value() else {
        panic!()
    };
    let Some(Record::Surface(moved)) = after.state().records.get(&surface.id) else {
        panic!()
    };
    assert_eq!(moved.desktop_rect.x, 20.);
    assert_ne!(moved.routing_token, surface.routing_token);
    assert!(after.state().revision > source.state().revision);
    assert_eq!(after.state().records.len(), source.state().records.len());
    drop(after_received);
    drop(before);
    drop(connection);
    drop(port);
    drop(owner);
    assert_eq!(budget.used(), 0);
}
