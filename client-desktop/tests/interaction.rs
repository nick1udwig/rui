mod common;
use common::*;
use rui_client_desktop::{interaction::*, *};
use rui_protocol_core::commands::Arguments;
use rui_protocol_core::model::*;
use rui_protocol_core::resources::MemoryBudget;

fn fixture() -> State {
    let mut s = state([solid(
        1,
        rect(0.0, 0.0, 10.0, 10.0),
        color(1.0, 0.0, 0.0, 1.0),
    )]);
    let mut a = surface(10, 1, 2.0, 3.0, 10.0, 10.0);
    a.input_region = vec![rect(0.0, 0.0, 10.0, 10.0)];
    s.records.insert(10, Record::Surface(a));
    s.records.insert(
        20,
        Record::Semantic(SemanticNode {
            browser_input_type: None,
            id: 20,
            version: 3,
            role: SemanticRole::Button,
            label: None,
            description: None,
            value: None,
            states: Default::default(),
            children: vec![],
            relations: vec![],
            actions: vec![Action {
                id: "Activate".into(),
                schema_version: 1,
            }],
            text: None,
            collection: None,
            bindings: vec![],
            semantic_group: None,
            reading_order: None,
            sensitivity: Sensitivity::Public,
            completeness: Completeness::Full,
        }),
    );
    s.records.insert(
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
            interaction_region: Some(rect(1.0, 1.0, 2.0, 2.0)),
        }),
    );
    s
}
#[test]
fn hit_test_inverts_only_local_viewport_and_surface_placement() {
    let s = fixture();
    let before = s.clone();
    let mut v = Viewport::new(100, 100);
    v.scale = 2.0;
    v.origin = point(1.0, 2.0);
    assert_eq!(
        hit_test(&s, 1, v, point(5.0, 5.0)).unwrap(),
        Target {
            surface: 10,
            semantic: 20
        }
    );
    assert_eq!(
        hit_test(&s, 1, v, point(0.0, 0.0)),
        Err(InteractionError::NoTarget)
    );
    assert_eq!(s, before);
    assert_eq!(
        hit_test(&s, 2, v, point(5.0, 5.0)),
        Err(InteractionError::StalePresentation)
    );
}
#[test]
fn modal_disabled_and_ambiguous_bindings_fail_closed() {
    let mut s = fixture();
    let v = Viewport::new(20, 20);
    let p = point(3.5, 4.5);
    let Record::Semantic(node) = s.records.get_mut(&20).unwrap() else {
        panic!()
    };
    node.states.insert(StateKey::Disabled, TriState::Unknown);
    assert_eq!(hit_test(&s, 1, v, p), Err(InteractionError::NoTarget));
    let Record::Semantic(node) = s.records.get_mut(&20).unwrap() else {
        panic!()
    };
    node.states.clear();
    let mut modal = surface(11, 1, 12.0, 12.0, 2.0, 2.0);
    modal.modal_for = Some(ObjectRef {
        kind: ObjectKind::Surface,
        id: 10,
    });
    s.records.insert(11, Record::Surface(modal));
    assert_eq!(hit_test(&s, 1, v, p), Err(InteractionError::NoTarget));
    s.records.remove(&11);
    let Record::Semantic(n) = s.records.get(&20).unwrap() else {
        panic!()
    };
    let mut n = n.clone();
    n.id = 21;
    s.records.insert(21, Record::Semantic(n));
    let Record::Binding(b) = s.records.get_mut(&30).unwrap() else {
        panic!()
    };
    b.semantics.push(ObjectRef {
        kind: ObjectKind::Semantic,
        id: 21,
    });
    assert_eq!(hit_test(&s, 1, v, p), Err(InteractionError::NoTarget));
}
#[test]
fn command_uses_declared_action_versions_and_explicit_authority() {
    let s = fixture();
    let binding = CommandBinding {
        epoch: [1; 16],
        view: [2; 16],
        client: [3; 16],
        lease: [4; 16],
        generation: 2,
        sequence: 9,
    };
    let c = command(&s, 20, "Activate", Arguments::None, binding).unwrap();
    assert_eq!(c.target_id, 20);
    assert_eq!(c.command_seq, 9);
    assert_eq!(c.observed_view_revision, 1);
    assert_eq!(c.preconditions.target_version, Some(3));
    assert_eq!(c.client_instance_id, [3; 16]);
    assert_eq!(
        command(
            &s,
            20,
            "Undeclared",
            Arguments::None,
            CommandBinding {
                epoch: [1; 16],
                view: [2; 16],
                client: [3; 16],
                lease: [4; 16],
                generation: 2,
                sequence: 9
            }
        ),
        Err(InteractionError::Unsupported)
    );
}
#[test]
fn display_pixels_own_charge_through_f64_conversion_and_release_on_drop() {
    let budget = MemoryBudget::new(1 << 20);
    let r = Renderer::new(RenderLimits::default(), budget.clone()).unwrap();
    let f = r
        .render_scene(
            &state([solid(
                1,
                rect(0.0, 0.0, 1.0, 1.0),
                color(1.0, 0.0, 0.0, 1.0),
            )]),
            reference(1),
            Viewport::new(1, 1),
            &(),
        )
        .unwrap();
    assert_eq!(budget.used(), 288);
    let rgba = f.into_rgba8().unwrap();
    assert_eq!(rgba.as_ref(), [255, 0, 0, 255]);
    assert_eq!(budget.used(), 8192 + 8);
    drop(rgba);
    assert_eq!(budget.used(), 0);
}

fn authority() -> CommandBinding {
    CommandBinding {
        epoch: [1; 16],
        view: [2; 16],
        client: [3; 16],
        lease: [4; 16],
        generation: 2,
        sequence: 9,
    }
}
fn editing_fixture() -> State {
    let mut s = fixture();
    let Record::Semantic(node) = s.records.get_mut(&20).unwrap() else {
        panic!()
    };
    node.text = Some(ObjectRef {
        kind: ObjectKind::Text,
        id: 40,
    });
    node.actions
        .extend(["Focus", "ReplaceText", "SetSelection"].map(|id| Action {
            id: id.into(),
            schema_version: 1,
        }));
    s.records.insert(
        40,
        Record::Text(TextModel {
            id: 40,
            version: 4,
            text_version: 3,
            status: TextStatus::Available,
            content: Some("Aé🦀Z".into()),
            selection: Some(TextRange {
                anchor: 3,
                focus: 1,
            }),
            editable: true,
            selectable: true,
        }),
    );
    s.records.insert(
        50,
        Record::Session(SessionState {
            id: 50,
            version: 2,
            canvas: rect(0.0, 0.0, 20.0, 20.0),
            cursor: None,
            focus: Some(ObjectRef {
                kind: ObjectKind::Semantic,
                id: 20,
            }),
            focus_surface: Some(ObjectRef {
                kind: ObjectKind::Surface,
                id: 10,
            }),
            focus_generation: 5,
        }),
    );
    s
}
#[test]
fn captured_commit_keeps_scalar_range_and_original_observed_revision() {
    let mut s = editing_fixture();
    let captured = editing_context(&s, 20, &authority()).unwrap();
    // An unrelated scene replacement may advance the view while editing clocks stay fixed.
    s.revision = 2;
    let Record::Scene(scene) = s.records.get_mut(&1).unwrap() else {
        panic!()
    };
    scene.version += 1;
    assert_eq!(editing_context(&s, 20, &authority()), Some(captured));
    let c = captured_text_command(&s, captured, "ß".into(), authority()).unwrap();
    assert_eq!(c.observed_view_revision, 1);
    assert_eq!(c.preconditions.text_version, Some(3));
    assert_eq!(c.preconditions.focus_generation, Some(5));
    assert_eq!(
        c.arguments,
        Arguments::ReplaceText {
            start: 1,
            end: 3,
            text: "ß".into()
        }
    );
    assert!(command(&s, 20, "Focus", Arguments::None, authority()).is_ok());
}
#[test]
fn every_relevant_source_and_authority_clock_invalidates_captured_commit() {
    let original = editing_fixture();
    let captured = editing_context(&original, 20, &authority()).unwrap();
    for case in 0..10 {
        let mut s = original.clone();
        let mut binding = authority();
        match case {
            0 => {
                let Record::Semantic(n) = s.records.get_mut(&20).unwrap() else {
                    panic!()
                };
                n.version += 1;
            }
            1 => {
                let Record::Text(t) = s.records.get_mut(&40).unwrap() else {
                    panic!()
                };
                t.text_version += 1;
            }
            2 => {
                let Record::Text(t) = s.records.get_mut(&40).unwrap() else {
                    panic!()
                };
                t.version += 1;
            }
            3 => {
                let Record::Text(t) = s.records.get_mut(&40).unwrap() else {
                    panic!()
                };
                t.selection = Some(TextRange {
                    anchor: 0,
                    focus: 0,
                });
            }
            4 => {
                let Record::Session(v) = s.records.get_mut(&50).unwrap() else {
                    panic!()
                };
                v.focus_generation += 1;
            }
            5 => {
                let Record::Session(v) = s.records.get_mut(&50).unwrap() else {
                    panic!()
                };
                v.focus = None;
            }
            6 => binding.lease = [6; 16],
            7 => binding.generation += 1,
            8 => binding.epoch = [8; 16],
            9 => binding.view = [9; 16],
            _ => unreachable!(),
        }
        assert_eq!(
            captured_text_command(&s, captured, "x".into(), binding),
            Err(InteractionError::StalePresentation),
            "case {case}"
        );
    }
}
#[test]
fn invalidation_precedes_reset_callbacks_and_late_commits_cannot_retarget() {
    let s = editing_fixture();
    let original = editing_context(&s, 20, &authority()).unwrap();
    let mut local = LocalComposition::default();
    local.install(Some(original));
    local.begin();
    local.invalidate();
    // Simulate a synchronous commit emitted by IM.reset.
    assert_eq!(local.take_commit(), None);
    let mut replacement = original;
    replacement.target = 99;
    local.install(Some(replacement));
    // A late old signal after reset is not armed by a physical key in this context.
    assert_eq!(local.take_commit(), None);
    local.install(Some(replacement));
    local.begin();
    assert_eq!(local.take_commit(), Some(replacement));
    assert_eq!(local.take_commit(), None);
}

#[test]
fn stable_item_activation_uses_unique_published_collection_owner_clock() {
    let mut s = fixture();
    let Record::Semantic(item) = s.records.get(&20).unwrap() else {
        panic!()
    };
    let mut list = item.clone();
    list.id = 60;
    list.role = SemanticRole::List;
    list.actions.clear();
    list.children = vec![ObjectRef {
        kind: ObjectKind::Semantic,
        id: 20,
    }];
    list.collection = Some(CollectionModel {
        version: 7,
        items: list.children.clone(),
        total_count: Some(10),
        materialization: Materialization::Partial,
        selected_items: vec![],
    });
    s.records.insert(60, Record::Semantic(list.clone()));
    let command = command(&s, 20, "Activate", Arguments::None, authority()).unwrap();
    assert_eq!(command.preconditions.model_version, Some(7));
    list.id = 61;
    s.records.insert(61, Record::Semantic(list));
    assert_eq!(
        rui_client_desktop::interaction::command(&s, 20, "Activate", Arguments::None, authority()),
        Err(InteractionError::Unsupported)
    );
    s.records.remove(&60);
    s.records.remove(&61);
    let Record::Semantic(item) = s.records.get_mut(&20).unwrap() else {
        panic!()
    };
    item.role = SemanticRole::ListItem;
    assert_eq!(
        rui_client_desktop::interaction::command(&s, 20, "Activate", Arguments::None, authority()),
        Err(InteractionError::Unsupported)
    );
}

#[test]
fn value_and_text_parameters_fail_before_building_a_command() {
    let mut s = fixture();
    let binding = CommandBinding {
        epoch: [1; 16],
        view: [2; 16],
        client: [3; 16],
        lease: [4; 16],
        generation: 1,
        sequence: 1,
    };
    let Record::Semantic(node) = s.records.get_mut(&20).unwrap() else {
        panic!()
    };
    node.role = SemanticRole::Checkbox;
    node.value = Some(TypedValue::Boolean(false));
    node.actions = vec![Action {
        id: "SetValue".into(),
        schema_version: 1,
    }];
    assert!(command(&s, 20, "SetValue", Arguments::Checked(true), binding).is_ok());
    for value in [
        Arguments::Number(f64::NAN),
        Arguments::Number(1.),
        Arguments::ValueText("true".into()),
    ] {
        assert_eq!(
            command(&s, 20, "SetValue", value, binding).unwrap_err(),
            InteractionError::Unsupported
        );
    }
    let Record::Semantic(node) = s.records.get_mut(&20).unwrap() else {
        panic!()
    };
    node.actions[0].schema_version = 2;
    assert_eq!(
        command(&s, 20, "SetValue", Arguments::Checked(true), binding).unwrap_err(),
        InteractionError::Unsupported
    );
}
