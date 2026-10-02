use rui_protocol_core::input::*;
use rui_protocol_core::model::{Affine, Point, Rect};

fn bounds() -> Rect {
    Rect {
        x: 0.0,
        y: 0.0,
        width: 100.0,
        height: 50.0,
    }
}
fn point() -> Point {
    Point { x: 10.0, y: 20.0 }
}
fn route() -> Route {
    Route {
        surface: 1,
        token: [1; 16],
        presented_revision: 7,
        bounds: bounds(),
        mapped: true,
        focused: true,
        blocked_by_modal: false,
        authorized: true,
        text_owner: TextOwner::HostIme,
    }
}
fn batch(events: Vec<Event>) -> InputBatch {
    InputBatch {
        surface: 1,
        routing_token: [1; 16],
        presented_revision: 7,
        events: events
            .into_iter()
            .enumerate()
            .map(|(i, event)| TimedEvent {
                timestamp_us: i as u64,
                event,
            })
            .collect(),
    }
}
fn router() -> InputRouter {
    InputRouter::new(1, InputLimits::default()).unwrap()
}

#[test]
fn inp_001_inverse_letterbox_and_zoom_do_not_change_host_layout() {
    let transform = Affine {
        xx: 1.5,
        yx: 0.0,
        xy: 0.0,
        yy: 1.5,
        tx: 20.0,
        ty: 10.0,
    };
    let clip = Rect {
        x: 0.0,
        y: 0.0,
        width: 200.0,
        height: 100.0,
    };
    assert_eq!(
        inverse_pointer(Point { x: 35.0, y: 40.0 }, transform, clip, bounds()).unwrap(),
        point()
    );
    assert_eq!(
        inverse_pointer(Point { x: 5.0, y: 5.0 }, transform, clip, bounds()),
        Err(InputError::Outside)
    );
    assert_eq!(
        inverse_pointer(
            point(),
            Affine {
                xx: 0.0,
                yy: 0.0,
                ..transform
            },
            clip,
            bounds()
        ),
        Err(InputError::Geometry)
    );
    assert_eq!(bounds().width, 100.0);
}
#[test]
fn inp_005_granted_region_mapping_is_explicit_and_half_open() {
    let granted = Rect {
        x: 400.0,
        y: 200.0,
        width: 300.0,
        height: 100.0,
    };
    assert_eq!(
        map_granted_region(point(), bounds(), granted).unwrap(),
        Point { x: 430.0, y: 240.0 }
    );
    assert_eq!(
        map_granted_region(Point { x: 100.0, y: 0.0 }, bounds(), granted),
        Err(InputError::Outside)
    );
    assert_eq!(
        map_granted_region(
            Point {
                x: f64::NAN,
                y: 0.0
            },
            bounds(),
            granted
        ),
        Err(InputError::Geometry)
    );
}
#[test]
fn inp_002_stale_route_or_modal_rejects_entire_batch_before_dispatch() {
    let mut router = router();
    let mut calls = 0;
    let events = batch(vec![
        Event::Motion(point()),
        Event::Button {
            code: 1,
            pressed: true,
            position: point(),
        },
    ]);
    let mut changed = route();
    changed.token = [2; 16];
    assert_eq!(
        router.dispatch(&changed, &events, |_| {
            calls += 1;
            Ok(())
        }),
        Err(InputError::StaleRoute)
    );
    changed = route();
    changed.blocked_by_modal = true;
    assert_eq!(
        router.dispatch(&changed, &events, |_| {
            calls += 1;
            Ok(())
        }),
        Err(InputError::Unauthorized)
    );
    assert_eq!(calls, 0);
    assert_eq!(router.held(), &HeldInput::default());
}
#[test]
fn inp_003_drag_preserves_every_move_and_does_not_coalesce() {
    let mut router = router();
    let mut events = vec![Event::Button {
        code: 1,
        pressed: true,
        position: point(),
    }];
    events.extend((0..100).map(|i| {
        Event::Motion(Point {
            x: i as f64,
            y: 20.0,
        })
    }));
    // Captured drag can leave the surface, but still needs its current route.
    events.push(Event::Motion(Point { x: 150.0, y: 20.0 }));
    events.push(Event::Button {
        code: 1,
        pressed: false,
        position: point(),
    });
    let expected = batch(events);
    let mut received = Vec::new();
    router
        .dispatch(&route(), &expected, |e| {
            received.push(e.clone());
            Ok(())
        })
        .unwrap();
    assert_eq!(received, expected.events);
    assert_eq!(router.held(), &HeldInput::default());
}
#[test]
fn inp_004_held_cleanup_bypasses_route_and_preserves_ambiguous_ownership() {
    let mut router = router();
    router
        .dispatch(
            &route(),
            &batch(vec![Event::Key {
                code: 30,
                pressed: true,
            }]),
            |_| Ok(()),
        )
        .unwrap();
    let mut changed = route();
    changed.token = [2; 16];
    changed.authorized = false;
    changed.mapped = false;
    changed.focused = false;
    router
        .dispatch(
            &changed,
            &batch(vec![Event::Key {
                code: 30,
                pressed: false,
            }]),
            |_| Ok(()),
        )
        .unwrap();
    let events = batch(vec![
        Event::Button {
            code: 1,
            pressed: true,
            position: point(),
        },
        Event::Button {
            code: 1,
            pressed: false,
            position: point(),
        },
    ]);
    let mut calls = 0;
    assert_eq!(
        router.dispatch(&route(), &events, |_| {
            calls += 1;
            if calls == 2 {
                Err(())
            } else {
                Ok(())
            }
        }),
        Err(InputError::OutcomeUnknown)
    );
    assert!(router.held().buttons.contains(&1));
    assert_eq!(
        router.dispatch(&route(), &batch(vec![Event::Motion(point())]), |_| Ok(())),
        Err(InputError::OutcomeUnknown)
    );
    assert!(router.drain_cleanup().buttons.contains(&1));
    assert_eq!(router.held(), &HeldInput::default());
}
#[test]
fn callback_panic_retains_cleanup_and_blocks_fresh_input() {
    let mut router = router();
    let events = batch(vec![Event::Key {
        code: 30,
        pressed: true,
    }]);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        router.dispatch(&route(), &events, |_| panic!("injected adapter failure"))
    }));
    assert!(panic.is_err());
    assert!(router.held().keys.contains(&30));
    assert!(router.drain_cleanup().keys.contains(&30));
}
#[test]
fn inp_006_physical_keys_never_generate_repeat_or_duplicate_client_text() {
    let mut router = router();
    let down = Event::Key {
        code: 30,
        pressed: true,
    };
    assert_eq!(
        router.dispatch(
            &route(),
            &batch(vec![down.clone(), down.clone()]),
            |_| panic!("invalid batch must be atomic")
        ),
        Err(InputError::Transition)
    );
    let mut client_text = route();
    client_text.text_owner = TextOwner::ClientText;
    assert_eq!(
        router.dispatch(&client_text, &batch(vec![down]), |_| panic!()),
        Err(InputError::TextOwnership)
    );
}
#[test]
fn timestamps_bounds_and_touch_lifecycle_fail_before_external_work() {
    let mut router = router();
    let mut reordered = batch(vec![Event::Motion(point()), Event::Motion(point())]);
    reordered.events[0].timestamp_us = 2;
    assert_eq!(
        router.dispatch(&route(), &reordered, |_| panic!()),
        Err(InputError::Timestamp)
    );
    assert_eq!(
        router.dispatch(
            &route(),
            &batch(vec![Event::TouchMove {
                id: 1,
                position: point()
            }]),
            |_| panic!()
        ),
        Err(InputError::Transition)
    );
    router
        .dispatch(
            &route(),
            &batch(vec![
                Event::TouchDown {
                    id: 1,
                    position: point(),
                },
                Event::TouchMove {
                    id: 1,
                    position: point(),
                },
                Event::TouchCancel { id: 1 },
            ]),
            |_| Ok(()),
        )
        .unwrap();
    assert!(router.held().touches.is_empty());
    let mut tiny = InputRouter::new(
        1,
        InputLimits {
            max_held: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        tiny.dispatch(
            &route(),
            &batch(vec![
                Event::Key {
                    code: 30,
                    pressed: true
                },
                Event::Key {
                    code: 31,
                    pressed: true
                }
            ]),
            |_| panic!()
        ),
        Err(InputError::Limit)
    );
}
fn context() -> EditingContext {
    EditingContext {
        target: 42,
        focus_generation: 3,
        text_version: 7,
        scalar_count: 6,
        owner: TextOwner::ClientText,
    }
}
#[test]
fn txt_002_preedit_is_transient_and_commit_produces_one_scalar_edit() {
    let mut machine = CompositionMachine::new(8, 64).unwrap();
    machine.set_context(Some(context()));
    machine
        .begin([1; 16], 3, 7, ScalarRange { start: 1, end: 2 })
        .unwrap();
    machine
        .update([1; 16], "😀e\u{301}אב", ScalarRange { start: 0, end: 5 })
        .unwrap();
    assert_eq!(machine.preedit(), Some("😀e\u{301}אב"));
    assert_eq!(
        machine.preedit_selection(),
        Some(ScalarRange { start: 0, end: 5 })
    );
    let edit = machine.commit([1; 16], 3, 7, "🦀").unwrap();
    assert_eq!(edit.target, 42);
    assert_eq!(edit.range, ScalarRange { start: 1, end: 2 });
    assert_eq!(edit.text, "🦀");
    assert!(machine.preedit().is_none());
    assert!(matches!(
        machine.commit([1; 16], 3, 7, "🦀"),
        Err(InputError::StaleComposition)
    ));
    assert_eq!(
        machine.begin([1; 16], 3, 7, ScalarRange { start: 1, end: 2 }),
        Err(InputError::StaleComposition)
    );
}
#[test]
fn txt_003_focus_text_node_owner_and_disconnect_changes_cancel_composition() {
    for changed in [
        None,
        Some(EditingContext {
            target: 43,
            ..context()
        }),
        Some(EditingContext {
            focus_generation: 4,
            ..context()
        }),
        Some(EditingContext {
            text_version: 8,
            ..context()
        }),
        Some(EditingContext {
            owner: TextOwner::HostIme,
            ..context()
        }),
    ] {
        let mut machine = CompositionMachine::new(8, 64).unwrap();
        machine.set_context(Some(context()));
        machine
            .begin([1; 16], 3, 7, ScalarRange { start: 0, end: 0 })
            .unwrap();
        assert_eq!(machine.set_context(changed), Some([1; 16]));
        assert!(matches!(
            machine.commit([1; 16], 3, 7, "late"),
            Err(InputError::StaleComposition)
        ));
    }
    let mut machine = CompositionMachine::new(1, 4).unwrap();
    machine.set_context(Some(context()));
    machine
        .begin([1; 16], 3, 7, ScalarRange { start: 0, end: 0 })
        .unwrap();
    assert_eq!(
        machine.update([1; 16], "too long", ScalarRange { start: 0, end: 0 }),
        Err(InputError::Limit)
    );
    assert_eq!(machine.cancel(), Some([1; 16]));
    assert_eq!(
        machine.begin([2; 16], 3, 7, ScalarRange { start: 0, end: 0 }),
        Err(InputError::Retention)
    );
}

#[test]
fn capture_allows_only_held_stream_continuations_outside_surface() {
    let mut router = router();
    let outside = Point { x: 150.0, y: 20.0 };
    router
        .dispatch(
            &route(),
            &batch(vec![Event::TouchDown {
                id: 1,
                position: point(),
            }]),
            |_| Ok(()),
        )
        .unwrap();
    for event in [
        Event::TouchDown {
            id: 2,
            position: outside,
        },
        Event::Button {
            code: 1,
            pressed: true,
            position: outside,
        },
        Event::Wheel {
            x: 0.0,
            y: 1.0,
            position: outside,
        },
        Event::Motion(outside),
    ] {
        assert_eq!(
            router.dispatch(&route(), &batch(vec![event]), |_| panic!(
                "fresh outside event"
            )),
            Err(InputError::Outside)
        );
    }
    router
        .dispatch(
            &route(),
            &batch(vec![Event::TouchMove {
                id: 1,
                position: outside,
            }]),
            |_| Ok(()),
        )
        .unwrap();
    assert_eq!(router.held().touches.len(), 1);
}
