mod common;
use common::*;
use prost::Message;
use rui_client_desktop::{
    interaction::CommandBinding,
    physical_dispatch::*,
    physical_input::{PhysicalInputError, PhysicalKey},
    source_controls::DeviceScope,
    Viewport,
};
use rui_protocol_core::{
    commands::{decode_family, Command, CommandFamily},
    input::{Event, TimedEvent},
    model::*,
    resources::MemoryBudget,
};
fn current() -> PhysicalControl {
    PhysicalControl {
        connection_generation: 2,
        grant_serial: 3,
    }
}
fn device() -> DeviceScope<'static> {
    DeviceScope {
        seat: "advertised-seat",
        device: "advertised-device",
        key_namespace: "linux.evdev",
        input_mode: "raw.host.v1",
        text_owner: "HOST_PHYSICAL",
    }
}
fn binding() -> CommandBinding {
    CommandBinding {
        epoch: [1; 16],
        view: [2; 16],
        client: [3; 16],
        lease: [4; 16],
        generation: 5,
        sequence: 6,
    }
}
fn fixture() -> State {
    let mut state = state([solid(
        1,
        rect(0.0, 0.0, 20.0, 20.0),
        color(0.0, 0.0, 1.0, 1.0),
    )]);
    let mut s = surface(10, 1, 2.0, 3.0, 20.0, 20.0);
    s.focused = true;
    s.routing_token = vec![7; 16];
    s.input_region = vec![rect(0.0, 0.0, 20.0, 20.0)];
    state.records.insert(10, Record::Surface(s));
    state.records.insert(
        40,
        Record::Session(SessionState {
            id: 40,
            version: 1,
            canvas: rect(0.0, 0.0, 100.0, 100.0),
            cursor: None,
            focus: None,
            focus_surface: Some(ObjectRef {
                kind: ObjectKind::Surface,
                id: 10,
            }),
            focus_generation: 1,
        }),
    );
    state
}
#[test]
fn physical_keys_bind_painted_focus_but_allow_current_actual_marker_across_visual_only_effects() {
    let mut state = fixture();
    let budget = MemoryBudget::new(64 * 1024);
    let painted = PresentedRouting::capture(&state, &budget).unwrap();
    let captured = CapturedPhysicalInput::key(
        painted.clone(),
        current(),
        Viewport::new(100, 100),
        123,
        PhysicalKey {
            code: 21,
            pressed: true,
        },
        &budget,
    )
    .unwrap();
    state.revision = 2;
    if let Record::Surface(s) = state.records.get_mut(&10).unwrap() {
        s.opacity = 0.5;
        s.source_capture_tokens = vec![vec![9]];
        s.version = 2;
    }
    let prepared = captured
        .prepare(&state, binding(), 2, current(), None, device())
        .unwrap();
    let Command::RawInput(decoded) =
        decode_family(CommandFamily::RawInput, &prepared.wire.encode_to_vec()).unwrap()
    else {
        panic!()
    };
    assert_eq!(decoded.header.command_seq, 6);
    assert_eq!(decoded.batch.presented_revision, 2);
    assert_eq!(
        decoded.batch.events[0],
        TimedEvent {
            timestamp_us: 123,
            event: Event::Key {
                code: 21,
                pressed: true
            }
        }
    );
    drop(prepared);
    let release = CapturedPhysicalInput::key(
        painted.clone(),
        current(),
        Viewport::new(100, 100),
        124,
        PhysicalKey {
            code: 21,
            pressed: false,
        },
        &budget,
    )
    .unwrap();
    let prepared = release
        .prepare(&state, binding(), 1, current(), None, device())
        .unwrap();
    assert_eq!(prepared.wire.presented_revision, 1);
    drop(prepared);
    let captured = CapturedPhysicalInput::key(
        painted.clone(),
        current(),
        Viewport::new(100, 100),
        125,
        PhysicalKey {
            code: 21,
            pressed: true,
        },
        &budget,
    )
    .unwrap();
    let mut other = surface(11, 1, 2.0, 3.0, 20.0, 20.0);
    other.focused = true;
    other.routing_token = vec![8; 16];
    state.records.insert(11, Record::Surface(other));
    if let Record::Surface(s) = state.records.get_mut(&10).unwrap() {
        s.focused = false;
    }
    if let Record::Session(s) = state.records.get_mut(&40).unwrap() {
        s.focus_surface = Some(ObjectRef {
            kind: ObjectKind::Surface,
            id: 11,
        });
        s.focus_generation = 2;
    }
    assert!(
        matches!(
            captured.prepare(&state, binding(), 2, current(), None, device()),
            Err(PhysicalInputError::OwnerChanged)
        ),
        "a key must not bind the newly focused, unpainted surface"
    );
    assert_eq!(painted.keyboard_owner(current()).unwrap().surface, 10);
    assert_eq!(keyboard_owner(&state, current()).unwrap().surface, 11);
    drop(painted);
    assert_eq!(budget.used(), 0);
}
#[test]
fn inverse_pointer_mapping_and_original_held_surface_survive_other_window_stacking() {
    let mut state = fixture();
    let budget = MemoryBudget::new(64 * 1024);
    let painted = PresentedRouting::capture(&state, &budget).unwrap();
    let mut viewport = Viewport::new(200, 200);
    viewport.scale = 1.5;
    let make = |event, capture| {
        CapturedPhysicalInput::pointer(
            painted.clone(),
            current(),
            viewport,
            TimedEvent {
                timestamp_us: 10,
                event,
            },
            &budget,
            capture,
        )
        .unwrap()
    };
    let press = make(
        Event::Button {
            code: 1,
            pressed: true,
            position: Point { x: 9.0, y: 12.0 },
        },
        None,
    )
    .prepare(&state, binding(), 1, current(), None, device())
    .unwrap();
    assert_eq!(
        press.event,
        Event::Button {
            code: 1,
            pressed: true,
            position: Point { x: 4.0, y: 5.0 }
        }
    );
    let owner = press.owner;
    drop(press);
    let mut other = surface(11, 1, 2.0, 3.0, 20.0, 20.0);
    other.stacking_order = 10;
    other.routing_token = vec![9; 16];
    other.input_region = vec![rect(0.0, 0.0, 20.0, 20.0)];
    state.records.insert(11, Record::Surface(other));
    let release = make(
        Event::Button {
            code: 1,
            pressed: false,
            position: Point { x: 9.0, y: 12.0 },
        },
        Some(owner),
    )
    .prepare(&state, binding(), 1, current(), Some(owner), device())
    .unwrap();
    assert_eq!(release.owner.surface, 10);
    drop(release);
    let stale = make(Event::Motion(Point { x: 9.0, y: 12.0 }), None);
    let mut fresh = current();
    fresh.grant_serial += 1;
    assert!(matches!(
        stale.prepare(&state, binding(), 1, fresh, Some(owner), device()),
        Err(PhysicalInputError::OwnerChanged)
    ));
    drop(painted);
    assert_eq!(budget.used(), 0);
}
#[test]
fn a_painted_pointer_press_never_retargets_after_occlusion_or_geometry_change() {
    let mut state = fixture();
    let budget = MemoryBudget::new(64 * 1024);
    let painted = PresentedRouting::capture(&state, &budget).unwrap();
    let viewport = Viewport::new(100, 100);
    let make = || {
        CapturedPhysicalInput::pointer(
            painted.clone(),
            current(),
            viewport,
            TimedEvent {
                timestamp_us: 10,
                event: Event::Button {
                    code: 1,
                    pressed: true,
                    position: Point { x: 6.0, y: 8.0 },
                },
            },
            &budget,
            None,
        )
        .unwrap()
    };
    let press = make();
    assert_eq!(press.captured_owner().surface, 10);
    let mut other = surface(11, 1, 2.0, 3.0, 20.0, 20.0);
    other.stacking_order = 10;
    other.routing_token = vec![9; 16];
    other.input_region = vec![rect(0.0, 0.0, 20.0, 20.0)];
    state.records.insert(11, Record::Surface(other));
    state.revision = 2;
    assert!(
        press
            .prepare(&state, binding(), 2, current(), None, device())
            .is_err(),
        "new topmost window cannot receive the older painted press"
    );
    state.records.remove(&11);
    let press = make();
    if let Record::Surface(s) = state.records.get_mut(&10).unwrap() {
        s.desktop_rect.x = 3.0;
    }
    assert!(
        press
            .prepare(&state, binding(), 2, current(), None, device())
            .is_err(),
        "geometry must match even if the producer incorrectly reused its token"
    );
    drop(painted);
    assert_eq!(budget.used(), 0);
}
#[test]
fn missing_presentation_wrong_mode_and_memory_exhaustion_cannot_prepare_input() {
    let state = fixture();
    let budget = MemoryBudget::new(64 * 1024);
    let painted = PresentedRouting::capture(&state, &budget).unwrap();
    let make = || {
        CapturedPhysicalInput::key(
            painted.clone(),
            current(),
            Viewport::new(100, 100),
            123,
            PhysicalKey {
                code: 30,
                pressed: true,
            },
            &budget,
        )
        .unwrap()
    };
    assert!(make()
        .prepare(&state, binding(), 0, current(), None, device())
        .is_err());
    let mut wrong = device();
    wrong.text_owner = "CLIENT_TEXT";
    assert!(make()
        .prepare(&state, binding(), 1, current(), None, wrong)
        .is_err());
    assert!(CapturedPhysicalInput::key(
        painted.clone(),
        current(),
        Viewport::new(100, 100),
        123,
        PhysicalKey {
            code: 30,
            pressed: true
        },
        &MemoryBudget::new(4095)
    )
    .is_err());
    drop(painted);
    assert_eq!(budget.used(), 0);
}
#[test]
fn new_modality_blocks_old_routes_and_summary_lifetimes_share_a_bounded_reservation() {
    let mut state = fixture();
    let budget = MemoryBudget::new(64 * 1024);
    let painted = PresentedRouting::capture(&state, &budget).unwrap();
    let charged = budget.used();
    assert!(charged > 0);
    let held = painted.clone();
    drop(painted);
    assert_eq!(budget.used(), charged);
    let old = held.keyboard_owner(current()).unwrap();
    let mut modal = surface(11, 1, 0.0, 0.0, 20.0, 20.0);
    modal.modal_for = Some(ObjectRef {
        kind: ObjectKind::Surface,
        id: 10,
    });
    modal.routing_token = vec![9; 16];
    state.records.insert(11, Record::Surface(modal));
    assert_eq!(keyboard_owner(&state, current()), None);
    assert!(!pointer_owner_is_current(&state, old));
    drop(held);
    assert_eq!(budget.used(), 0);
    if let Record::Surface(s) = state.records.get_mut(&10).unwrap() {
        s.input_region = vec![rect(0.0, 0.0, 1.0, 1.0); 65];
    }
    assert!(PresentedRouting::capture(&state, &budget).is_err());
    assert_eq!(budget.used(), 0);
    assert!(PresentedRouting::capture(&fixture(), &MemoryBudget::new(1)).is_err());
}

#[test]
fn an_unrouteable_top_surface_never_falls_through_to_the_window_below() {
    let mut state = fixture();
    let budget = MemoryBudget::new(64 * 1024);
    let painted = PresentedRouting::capture(&state, &budget).unwrap();
    let old = CapturedPhysicalInput::pointer(
        painted.clone(),
        current(),
        Viewport::new(100, 100),
        TimedEvent {
            timestamp_us: 1,
            event: Event::Button {
                code: 1,
                pressed: true,
                position: Point { x: 6.0, y: 8.0 },
            },
        },
        &budget,
        None,
    )
    .unwrap();
    let mut overlay = surface(11, 1, 2.0, 3.0, 20.0, 20.0);
    overlay.stacking_order = 10;
    overlay.input_region = vec![rect(0.0, 0.0, 20.0, 20.0)];
    overlay.routing_token = vec![9; 16];
    overlay.visual_coverage = VisualCoverage::Partial;
    state.records.insert(11, Record::Surface(overlay));
    state.revision = 2;
    let partial = PresentedRouting::capture(&state, &budget).unwrap();
    assert_eq!(
        partial.pointer_owner(
            current(),
            Viewport::new(100, 100),
            Point { x: 6.0, y: 8.0 },
            None
        ),
        None
    );
    assert!(old
        .prepare(&state, binding(), 2, current(), None, device())
        .is_err());
    if let Record::Surface(s) = state.records.get_mut(&11).unwrap() {
        s.visual_coverage = VisualCoverage::Complete;
        s.routing_token = vec![0; 16];
    }
    let blocked = PresentedRouting::capture(&state, &budget).unwrap();
    assert_eq!(
        blocked.pointer_owner(
            current(),
            Viewport::new(100, 100),
            Point { x: 6.0, y: 8.0 },
            None
        ),
        None
    );
    drop(blocked);
    drop(partial);
    drop(painted);
    assert_eq!(budget.used(), 0);
}
