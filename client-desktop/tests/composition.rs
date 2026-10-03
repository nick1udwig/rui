mod common;
use common::*;
use rui_client_desktop::{composition::*, interaction::*};
use rui_protocol_core::{
    commands::CompositionOperation, input::ScalarRange, model::*, resources::*,
};

fn binding(sequence: u64) -> CommandBinding {
    CommandBinding {
        epoch: [1; 16],
        view: [2; 16],
        client: [3; 16],
        lease: [4; 16],
        generation: 1,
        sequence,
    }
}
fn fixture(content: &str, anchor: u64, focus: u64) -> State {
    let mut state = state([]);
    state.records.insert(
        20,
        Record::Semantic(SemanticNode {
            id: 20,
            version: 1,
            role: SemanticRole::TextField,
            label: None,
            description: None,
            value: None,
            states: Default::default(),
            children: vec![],
            relations: vec![],
            actions: vec![Action {
                id: "ReplaceText".into(),
                schema_version: 1,
            }],
            text: Some(ObjectRef {
                kind: ObjectKind::Text,
                id: 40,
            }),
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
        Record::Text(TextModel {
            id: 40,
            version: 1,
            text_version: 1,
            status: TextStatus::Available,
            content: Some(content.into()),
            selection: Some(TextRange { anchor, focus }),
            editable: true,
            selectable: true,
        }),
    );
    state.records.insert(
        50,
        Record::Session(SessionState {
            id: 50,
            version: 1,
            canvas: rect(0., 0., 20., 20.),
            cursor: None,
            focus: Some(ObjectRef {
                kind: ObjectKind::Semantic,
                id: 20,
            }),
            focus_surface: Some(ObjectRef {
                kind: ObjectKind::Surface,
                id: 60,
            }),
            focus_generation: 1,
        }),
    );
    let mut surface = surface(60, 1, 0., 0., 20., 20.);
    surface.source_capture_tokens = vec![b"before".to_vec()];
    state.records.insert(60, Record::Surface(surface));
    state
}
fn captured(state: &State) -> EditingContext {
    editing_context(state, 20, &binding(1)).unwrap()
}
fn begin(queue: &mut CompositionQueue, state: &State, id: [u8; 16], sequence: u64) {
    let context = queue.context().unwrap_or_else(|| captured(state));
    queue
        .accept(context, id, NativeStage::Begin, state, &binding(sequence))
        .unwrap();
    let prepared = queue
        .start(state, &binding(sequence), Some(state.revision))
        .unwrap()
        .unwrap();
    assert_eq!(prepared.command.text_session_id, id);
    assert!(matches!(
        prepared.command.operation,
        CompositionOperation::Begin(_)
    ));
    queue.result(sequence, "Dispatched", "", None).unwrap();
}
fn commit(queue: &mut CompositionQueue, state: &State, id: [u8; 16], text: &str, sequence: u64) {
    queue
        .accept(
            queue.context().unwrap(),
            id,
            NativeStage::Commit(text),
            state,
            &binding(sequence),
        )
        .unwrap();
    let prepared = queue
        .start(state, &binding(sequence), Some(state.revision))
        .unwrap()
        .unwrap();
    assert!(matches!(
        prepared.command.operation,
        CompositionOperation::CommitText { .. }
    ));
}
fn publish(
    state: &mut State,
    content: &str,
    caret: u64,
    text_delta: u64,
    record_delta: u64,
    cause: &str,
) {
    state.revision += 1;
    let Record::Text(text) = state.records.get_mut(&40).unwrap() else {
        panic!()
    };
    text.content = Some(content.into());
    text.selection = Some(TextRange {
        anchor: caret,
        focus: caret,
    });
    text.text_version += text_delta;
    text.version += record_delta;
    let Record::Semantic(node) = state.records.get_mut(&20).unwrap() else {
        panic!()
    };
    node.version += 1;
    let Record::Surface(surface) = state.records.get_mut(&60).unwrap() else {
        panic!()
    };
    surface.source_capture_tokens = vec![cause.as_bytes().to_vec()];
}
#[test]
fn transient_preedit_preserves_document_and_waits_for_native_dispatch_and_presentation() {
    let budget = MemoryBudget::new(1 << 20);
    let mut queue = CompositionQueue::new(budget.clone(), ClockPolicy::Strict).unwrap();
    let mut state = fixture("A😀Z", 2, 1);
    let context = captured(&state);
    queue
        .accept(context, [1; 16], NativeStage::Begin, &state, &binding(7))
        .unwrap();
    assert!(queue.start(&state, &binding(7), None).unwrap().is_none());
    assert!(queue.start(&state, &binding(7), Some(0)).unwrap().is_none());
    let prepared = queue.start(&state, &binding(7), Some(1)).unwrap().unwrap();
    assert!(matches!(
        prepared.command.operation,
        CompositionOperation::Begin(ScalarRange { start: 1, end: 2 })
    ));
    queue
        .accept(
            context,
            [1; 16],
            NativeStage::Update {
                text: "e\u{301}אב",
                selection: ScalarRange { start: 2, end: 4 },
            },
            &state,
            &binding(8),
        )
        .unwrap();
    assert!(queue.start(&state, &binding(8), Some(1)).unwrap().is_none());
    assert!(!queue.result(999, "Dispatched", "", None).unwrap());
    queue.result(7, "Dispatched", "", None).unwrap();
    let update = queue.start(&state, &binding(8), Some(1)).unwrap().unwrap();
    assert!(matches!(
        update.command.operation,
        CompositionOperation::UpdatePreedit { .. }
    ));
    queue.result(8, "Dispatched", "", None).unwrap();
    assert!(!queue.observe(&state, &binding(9)).unwrap());
    let Record::Text(text) = state.records.get(&40).unwrap() else {
        panic!()
    };
    assert_eq!(text.content.as_deref(), Some("A😀Z"));
    commit(&mut queue, &state, [1; 16], "é", 9);
    queue.result(9, "Applied", "own", Some(2)).unwrap();
    publish(&mut state, "AéZ", 2, 1, 1, "own");
    assert!(queue.observe(&state, &binding(10)).unwrap());
    drop(prepared);
    drop(update);
    drop(queue);
    assert_eq!(budget.used(), 0);
}
#[test]
fn gtk_negotiated_clock_contract_accepts_only_exact_native_patterns_in_both_orders() {
    for state_first in [false, true] {
        for (before, range, text, after, caret, content_delta, record_delta) in [
            ("abc", (0, 3), "😀e\u{301}", "😀e\u{301}", 3, 2, 3),
            ("abc", (1, 1), "אב", "aאבbc", 3, 2, 3),
            ("abc", (0, 3), "abc", "abc", 3, 2, 3),
            ("abc", (0, 1), "", "bc", 0, 2, 2),
            ("abc", (0, 3), "", "", 0, 1, 1),
            ("", (0, 0), "😀", "😀", 1, 1, 2),
            ("", (0, 0), "", "", 0, 0, 0),
        ] {
            let budget = MemoryBudget::new(1 << 20);
            let mut queue =
                CompositionQueue::new(budget.clone(), ClockPolicy::GtkEditable4224).unwrap();
            let mut state = fixture(before, range.0, range.1);
            begin(&mut queue, &state, [1; 16], 1);
            commit(&mut queue, &state, [1; 16], text, 2);
            if state_first {
                publish(&mut state, after, caret, content_delta, record_delta, "own");
                assert!(!queue.observe(&state, &binding(3)).unwrap());
                queue.result(2, "Applied", "own", Some(2)).unwrap();
            } else {
                queue.result(2, "Applied", "own", None).unwrap();
                // An unrelated visual frame cannot confirm a same-content/noop edit.
                state.revision += 1;
                assert!(!queue.observe(&state, &binding(3)).unwrap());
                publish(&mut state, after, caret, content_delta, record_delta, "own");
            }
            assert!(queue.observe(&state, &binding(3)).unwrap());
            assert_eq!(queue.pending_count(), 0);
            drop(queue);
            assert_eq!(budget.used(), 0);
        }
    }
}
#[test]
fn buffered_native_commits_rebase_only_after_own_cause_and_actual_new_presentation() {
    let budget = MemoryBudget::new(1 << 20);
    let mut queue = CompositionQueue::new(budget.clone(), ClockPolicy::GtkEditable4224).unwrap();
    let mut state = fixture("", 0, 0);
    let origin = captured(&state);
    for (id, text) in [([1; 16], "😀"), ([2; 16], "אב")] {
        queue
            .accept(origin, id, NativeStage::Begin, &state, &binding(1))
            .unwrap();
        queue
            .accept(origin, id, NativeStage::Commit(text), &state, &binding(1))
            .unwrap();
    }
    queue.start(&state, &binding(1), Some(1)).unwrap().unwrap();
    queue.result(1, "Dispatched", "", None).unwrap();
    queue.start(&state, &binding(2), Some(1)).unwrap().unwrap();
    publish(&mut state, "😀", 1, 1, 2, "first");
    queue.result(2, "Applied", "first", Some(2)).unwrap();
    assert!(queue.start(&state, &binding(3), Some(2)).unwrap().is_none());
    assert!(queue.observe(&state, &binding(3)).unwrap());
    assert!(queue.start(&state, &binding(3), Some(1)).unwrap().is_none());
    let prepared = queue.start(&state, &binding(3), Some(2)).unwrap().unwrap();
    assert_eq!(prepared.command.base_text_version, 2);
    assert!(matches!(
        prepared.command.operation,
        CompositionOperation::Begin(ScalarRange { start: 1, end: 1 })
    ));
    queue.result(3, "Dispatched", "", None).unwrap();
    queue.start(&state, &binding(4), Some(2)).unwrap().unwrap();
    publish(&mut state, "😀אב", 3, 2, 3, "second");
    queue.result(4, "Applied", "second", Some(3)).unwrap();
    assert!(queue.observe(&state, &binding(5)).unwrap());
    assert_eq!(queue.context(), Some(origin));
    drop(prepared);
    drop(queue);
    assert_eq!(budget.used(), 0);
}
#[test]
fn generic_clock_policy_does_not_infer_gtk_from_matching_cause() {
    let mut queue = CompositionQueue::new(MemoryBudget::new(1 << 20), ClockPolicy::Strict).unwrap();
    let mut state = fixture("abc", 0, 3);
    begin(&mut queue, &state, [1; 16], 1);
    commit(&mut queue, &state, [1; 16], "x", 2);
    publish(&mut state, "x", 1, 2, 3, "own");
    queue.result(2, "Applied", "own", Some(2)).unwrap();
    assert_eq!(
        queue.observe(&state, &binding(3)),
        Err(CompositionError::Context)
    );
}
#[test]
fn foreign_cause_clock_aba_focus_surface_and_authority_changes_fail_closed() {
    for case in 0..8 {
        let mut queue =
            CompositionQueue::new(MemoryBudget::new(1 << 20), ClockPolicy::GtkEditable4224)
                .unwrap();
        let mut state = fixture("abc", 0, 3);
        begin(&mut queue, &state, [1; 16], 1);
        commit(&mut queue, &state, [1; 16], "x", 2);
        publish(&mut state, "x", 1, 2, 3, "own");
        queue.result(2, "Applied", "own", Some(2)).unwrap();
        let mut authority = binding(3);
        match case {
            0 => {
                let Record::Text(text) = state.records.get_mut(&40).unwrap() else {
                    panic!()
                };
                text.text_version += 2;
            }
            1 => {
                let Record::Text(text) = state.records.get_mut(&40).unwrap() else {
                    panic!()
                };
                text.version += 1;
            }
            2 => {
                let Record::Session(session) = state.records.get_mut(&50).unwrap() else {
                    panic!()
                };
                session.focus_generation += 2;
            }
            3 => authority.generation += 1,
            4 => {
                let Record::Surface(surface) = state.records.get_mut(&60).unwrap() else {
                    panic!()
                };
                surface.source_capture_tokens = vec![b"foreign".to_vec()];
            }
            5 => {
                state.records.remove(&20);
            }
            6 => {
                let Record::Surface(surface) = state.records.get_mut(&60).unwrap() else {
                    panic!()
                };
                surface.mapped = false;
            }
            7 => {
                let Record::Text(text) = state.records.get_mut(&40).unwrap() else {
                    panic!()
                };
                text.selection = Some(TextRange {
                    anchor: 0,
                    focus: 0,
                });
            }
            _ => unreachable!(),
        }
        assert!(queue.observe(&state, &authority).is_err(), "case {case}");
        queue.cancel();
        assert_eq!(queue.pending_count(), 0);
    }
}
#[test]
fn exact_result_retry_agrees_and_unknown_never_prepares_a_replacement_effect() {
    let mut queue = CompositionQueue::new(MemoryBudget::new(1 << 20), ClockPolicy::Strict).unwrap();
    let state = fixture("", 0, 0);
    begin(&mut queue, &state, [1; 16], 1);
    commit(&mut queue, &state, [1; 16], "x", 2);
    assert!(!queue.result(99, "Applied", "other", Some(2)).unwrap());
    assert_eq!(
        queue.result(2, "OutcomeUnknown", "", None),
        Err(CompositionError::Result)
    );
    assert!(queue.start(&state, &binding(3), Some(1)).unwrap().is_none());
    queue.result(2, "Applied", "own", None).unwrap();
    queue.result(2, "Applied", "own", None).unwrap();
    assert_eq!(
        queue.result(2, "Applied", "other", None),
        Err(CompositionError::Result)
    );
}
#[test]
fn cancellation_releases_transient_memory_and_never_reuses_retired_ids() {
    let budget = MemoryBudget::new(1 << 20);
    let mut queue = CompositionQueue::new(budget.clone(), ClockPolicy::Strict).unwrap();
    let baseline = budget.used();
    let state = fixture("", 0, 0);
    for count in 1..=128u16 {
        let mut id = [0; 16];
        id[..2].copy_from_slice(&count.to_be_bytes());
        let context = captured(&state);
        queue
            .accept(context, id, NativeStage::Begin, &state, &binding(1))
            .unwrap();
        queue
            .accept(
                context,
                id,
                NativeStage::Update {
                    text: "😀",
                    selection: ScalarRange { start: 1, end: 1 },
                },
                &state,
                &binding(1),
            )
            .unwrap();
        assert!(budget.used() > baseline);
        if count % 2 == 0 {
            queue.start(&state, &binding(1), Some(1)).unwrap().unwrap();
            queue.result(1, "Dispatched", "", None).unwrap();
        }
        queue.cancel();
        assert_eq!(budget.used(), baseline);
        assert_eq!(
            queue.accept(context, id, NativeStage::Begin, &state, &binding(1)),
            Err(CompositionError::Stage)
        );
    }
    assert_eq!(
        queue.accept(
            captured(&state),
            [255; 16],
            NativeStage::Begin,
            &state,
            &binding(1)
        ),
        Err(CompositionError::Limit)
    );
    drop(queue);
    assert_eq!(budget.used(), 0);
}
#[test]
fn allocation_failure_does_not_publish_a_native_begin_or_retain_text() {
    let budget = MemoryBudget::new(1 << 20);
    let mut queue = CompositionQueue::new(budget.clone(), ClockPolicy::Strict).unwrap();
    let state = fixture("", 0, 0);
    let baseline = budget.used();
    let other = budget.reserve(budget.limit() - baseline).unwrap();
    assert_eq!(
        queue.accept(
            captured(&state),
            [1; 16],
            NativeStage::Begin,
            &state,
            &binding(1)
        ),
        Err(CompositionError::Memory)
    );
    assert_eq!(queue.pending_count(), 0);
    assert!(queue.context().is_none());
    drop(other);
    begin(&mut queue, &state, [1; 16], 1);
    let text = "x".repeat(MAX_COMPOSITION_BYTES + 1);
    assert_eq!(
        queue.accept(
            queue.context().unwrap(),
            [1; 16],
            NativeStage::Commit(&text),
            &state,
            &binding(2)
        ),
        Err(CompositionError::Limit)
    );
    queue
        .accept(
            queue.context().unwrap(),
            [1; 16],
            NativeStage::Cancel,
            &state,
            &binding(2),
        )
        .unwrap();
    queue.start(&state, &binding(2), Some(1)).unwrap().unwrap();
    queue.result(2, "Dispatched", "", None).unwrap();
    drop(queue);
    assert_eq!(budget.used(), 0);
}

#[test]
fn queued_text_and_event_bounds_are_enforced_before_retention() {
    let budget = MemoryBudget::new(1 << 20);
    let mut queue = CompositionQueue::new(budget.clone(), ClockPolicy::Strict).unwrap();
    let state = fixture("", 0, 0);
    let context = captured(&state);
    queue
        .accept(context, [1; 16], NativeStage::Begin, &state, &binding(1))
        .unwrap();
    let text = "x".repeat(MAX_COMPOSITION_BYTES);
    for _ in 0..4 {
        queue
            .accept(
                context,
                [1; 16],
                NativeStage::Update {
                    text: &text,
                    selection: ScalarRange { start: 0, end: 0 },
                },
                &state,
                &binding(1),
            )
            .unwrap();
    }
    let before = budget.used();
    assert_eq!(
        queue.accept(
            context,
            [1; 16],
            NativeStage::Commit("x"),
            &state,
            &binding(1)
        ),
        Err(CompositionError::Limit)
    );
    assert_eq!(budget.used(), before);
    queue.cancel();
    queue
        .accept(context, [2; 16], NativeStage::Begin, &state, &binding(1))
        .unwrap();
    assert_eq!(
        queue.accept(
            context,
            [2; 16],
            NativeStage::Update {
                text: "😀",
                selection: ScalarRange { start: 0, end: 2 }
            },
            &state,
            &binding(1)
        ),
        Err(CompositionError::Stage)
    );
    for _ in 0..31 {
        queue
            .accept(
                context,
                [2; 16],
                NativeStage::Update {
                    text: "",
                    selection: ScalarRange { start: 0, end: 0 },
                },
                &state,
                &binding(1),
            )
            .unwrap();
    }
    assert_eq!(
        queue.accept(context, [2; 16], NativeStage::Cancel, &state, &binding(1)),
        Err(CompositionError::Limit)
    );
    drop(queue);
    assert_eq!(budget.used(), 0);
}
#[test]
fn prepared_text_keeps_its_charge_after_queue_teardown() {
    let budget = MemoryBudget::new(1 << 20);
    let mut queue = CompositionQueue::new(budget.clone(), ClockPolicy::Strict).unwrap();
    let state = fixture("", 0, 0);
    begin(&mut queue, &state, [1; 16], 1);
    queue
        .accept(
            queue.context().unwrap(),
            [1; 16],
            NativeStage::Update {
                text: "private-preedit",
                selection: ScalarRange { start: 0, end: 0 },
            },
            &state,
            &binding(2),
        )
        .unwrap();
    let prepared = queue.start(&state, &binding(2), Some(1)).unwrap().unwrap();
    drop(queue);
    assert!(budget.used() >= "private-preedit".len() + 2048);
    drop(prepared);
    assert_eq!(budget.used(), 0);
}
