mod common;
use common::*;
use prost::Message;
use rui_client_desktop::{interaction::CommandBinding, source_controls::*, Viewport};
use rui_protocol_core::{
    commands::{decode_family, Command, CommandFamily},
    model::*,
    resources::MemoryBudget,
};
fn fixture() -> State {
    let mut state = state([solid(
        1,
        rect(0.0, 0.0, 20.0, 20.0),
        color(0.0, 0.0, 1.0, 1.0),
    )]);
    let mut a = surface(10, 1, 2.0, 3.0, 20.0, 20.0);
    a.input_region = vec![rect(0.0, 0.0, 20.0, 20.0)];
    a.routing_token = vec![7; 16];
    state.records.insert(10, Record::Surface(a));
    state.records.insert(
        20,
        Record::Semantic(SemanticNode {
            browser_input_type: None,
            id: 20,
            version: 1,
            role: SemanticRole::CustomCanvas,
            label: None,
            description: None,
            value: None,
            states: Default::default(),
            children: vec![],
            relations: vec![],
            actions: vec![],
            text: None,
            collection: None,
            bindings: vec![],
            semantic_group: None,
            reading_order: None,
            sensitivity: Sensitivity::Public,
            completeness: Completeness::Full,
        }),
    );
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
    state.records.insert(
        30,
        Record::Binding(Binding {
            id: 30,
            version: 1,
            surface: ObjectRef {
                kind: ObjectKind::Surface,
                id: 10,
            },
            semantics: vec![ObjectRef {
                kind: ObjectKind::Semantic,
                id: 20,
            }],
            scenes: vec![reference(1)],
            interaction_region: Some(rect(0.0, 0.0, 20.0, 20.0)),
        }),
    );
    state
}
#[test]
fn explicit_resize_uses_the_published_focused_toplevel_and_never_local_viewport_size() {
    let mut state = fixture();
    let budget = MemoryBudget::new(20_000);
    let target = resize_target(&state).unwrap();
    let prepared = resize_command(&state, target, 30.0, 25.0, binding(), &budget).unwrap();
    let Command::Resize(decoded) =
        decode_family(CommandFamily::Resize, &prepared.wire().encode_to_vec()).unwrap()
    else {
        panic!()
    };
    assert_eq!(
        (
            decoded.surface_id,
            decoded.surface_version,
            decoded.width,
            decoded.height
        ),
        (10, 1, 30.0, 25.0)
    );
    drop(prepared);
    assert_eq!(budget.used(), 0);
    for (width, height) in [
        (0.0, 1.0),
        (-1.0, 1.0),
        (f64::NAN, 1.0),
        (1.0, f64::INFINITY),
    ] {
        assert!(matches!(
            resize_command(&state, target, width, height, binding(), &budget),
            Err(SourceControlError::Geometry)
        ));
    }
    let Record::Surface(current) = state.records.get_mut(&10).unwrap() else {
        panic!()
    };
    current.version += 1;
    assert!(matches!(
        resize_command(&state, target, 30.0, 25.0, binding(), &budget),
        Err(SourceControlError::Stale)
    ));
    let mut modal = surface(11, 1, 50.0, 50.0, 20.0, 20.0);
    modal.modal_for = Some(ObjectRef {
        kind: ObjectKind::Surface,
        id: 10,
    });
    state.records.insert(11, Record::Surface(modal));
    assert_eq!(resize_target(&state), Err(SourceControlError::Target));
    assert_eq!(budget.used(), 0);
}
fn binding() -> CommandBinding {
    CommandBinding {
        epoch: [1; 16],
        view: [2; 16],
        client: [3; 16],
        lease: [4; 16],
        generation: 1,
        sequence: 1,
    }
}
fn device() -> DeviceScope<'static> {
    DeviceScope {
        seat: "advertised-seat",
        device: "advertised-device",
        key_namespace: "linux.evdev",
        input_mode: "raw.pointer.v1",
        text_owner: "CLIENT_TEXT",
    }
}
fn sample(n: u64, x: f64, y: f64) -> PointerSample {
    PointerSample {
        timestamp_us: n,
        point: point(x, y),
    }
}
#[test]
fn captured_stroke_maps_every_sample_and_retains_its_preallocation_charge() {
    let state = fixture();
    let before = state.clone();
    let budget = MemoryBudget::new(100_000);
    let mut viewport = Viewport::new(100, 100);
    viewport.scale = 2.0;
    viewport.origin = point(1.0, 2.0);
    let mut stroke = PointerStroke::begin(1, viewport, sample(10, 4.0, 4.0), &budget).unwrap();
    stroke.push(sample(11, 6.0, 8.0)).unwrap();
    stroke.push(sample(11, 8.0, 10.0)).unwrap();
    stroke.finish(sample(12, 10.0, 12.0)).unwrap();
    let prepared = stroke.into_command(&state, binding(), device()).unwrap();
    assert!(budget.used() > 0);
    let wire = prepared.wire();
    assert_eq!(wire.events.len(), 4);
    assert_eq!(
        (&*wire.seat_id, &*wire.device_id),
        ("advertised-seat", "advertised-device")
    );
    let Command::RawInput(decoded) =
        decode_family(CommandFamily::RawInput, &wire.encode_to_vec()).unwrap()
    else {
        panic!("family changed")
    };
    use rui_protocol_core::input::Event;
    assert!(
        matches!(decoded.batch.events[0].event,Event::Button{pressed:true,position,..} if position==point(1.0,1.0))
    );
    assert!(matches!(decoded.batch.events[1].event,Event::Motion(p) if p==point(2.0,3.0)));
    assert!(matches!(decoded.batch.events[2].event,Event::Motion(p) if p==point(3.0,4.0)));
    assert!(
        matches!(decoded.batch.events[3].event,Event::Button{pressed:false,position,..} if position==point(4.0,5.0))
    );
    assert_eq!(decoded.batch.routing_token, [7; 16]);
    assert_eq!(
        decoded
            .batch
            .events
            .iter()
            .map(|e| e.timestamp_us)
            .collect::<Vec<_>>(),
        vec![10, 11, 11, 12]
    );
    assert_eq!(state, before);
    drop(prepared);
    assert_eq!(budget.used(), 0);
}
#[test]
fn overflow_stale_modal_and_late_outside_samples_never_build_a_partial_batch() {
    let state = fixture();
    let budget = MemoryBudget::new(100_000);
    let viewport = Viewport::new(100, 100);
    assert!(matches!(
        PointerStroke::begin(1, viewport, sample(1, 5.0, 5.0), &MemoryBudget::new(1)),
        Err(SourceControlError::Limit)
    ));
    let mut stroke = PointerStroke::begin(1, viewport, sample(1, 5.0, 5.0), &budget).unwrap();
    for n in 1..MAX_STROKE_POINTS {
        stroke.push(sample(n as u64 + 1, 5.0, 5.0)).unwrap();
    }
    assert_eq!(
        stroke.finish(sample(100, 5.0, 5.0)),
        Err(SourceControlError::Limit)
    );
    assert!(matches!(
        stroke.into_command(&state, binding(), device()),
        Err(SourceControlError::Stale)
    ));
    for case in 0..3 {
        let mut stroke = PointerStroke::begin(1, viewport, sample(1, 5.0, 5.0), &budget).unwrap();
        stroke
            .finish(sample(2, if case == 2 { 40.0 } else { 6.0 }, 6.0))
            .unwrap();
        let mut changed = state.clone();
        if case == 0 {
            changed.revision = 2;
        } else if case == 1 {
            let mut modal = surface(11, 1, 50.0, 50.0, 20.0, 20.0);
            modal.modal_for = Some(ObjectRef {
                kind: ObjectKind::Surface,
                id: 10,
            });
            changed.records.insert(11, Record::Surface(modal));
        }
        assert!(stroke.into_command(&changed, binding(), device()).is_err());
    }
    assert_eq!(budget.used(), 0);
}
