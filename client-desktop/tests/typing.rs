mod common;
use common::*;
use rui_client_desktop::{interaction::*, typing::*};
use rui_protocol_core::{model::*, resources::*};

fn fixture(content: &str, selection: TextRange) -> State {
    let mut s = state([]);
    s.records.insert(
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
    s.records.insert(
        40,
        Record::Text(TextModel {
            id: 40,
            version: 1,
            text_version: 1,
            status: TextStatus::Available,
            content: Some(content.into()),
            selection: Some(selection),
            editable: true,
            selectable: true,
        }),
    );
    s.records.insert(
        50,
        Record::Session(SessionState {
            id: 50,
            version: 1,
            canvas: rect(0.0, 0.0, 20.0, 20.0),
            cursor: None,
            focus: Some(ObjectRef {
                kind: ObjectKind::Semantic,
                id: 20,
            }),
            focus_surface: None,
            focus_generation: 1,
        }),
    );
    s
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
fn accept(
    queue: &mut TypingQueue,
    s: &State,
    context: EditingContext,
    text: &str,
    budget: &MemoryBudget,
) {
    queue
        .accept(
            context,
            text.into(),
            budget.reserve(text.len() + 2048).unwrap(),
            s,
            &binding(),
        )
        .unwrap();
}
fn apply(s: &mut State, content: &str, selection: TextRange) {
    s.revision += 1;
    let Record::Text(t) = s.records.get_mut(&40).unwrap() else {
        panic!()
    };
    if t.content.as_deref() != Some(content) {
        t.text_version += 1;
    }
    t.version += 1;
    t.content = Some(content.into());
    t.selection = Some(selection);
    let Record::Semantic(n) = s.records.get_mut(&20).unwrap() else {
        panic!()
    };
    n.version += 1;
}
#[test]
fn rapid_commits_preserve_scalar_order_and_require_both_result_and_source() {
    let budget = MemoryBudget::new(1 << 20);
    let mut q = TypingQueue::new(budget.clone()).unwrap();
    let mut s = fixture(
        "",
        TextRange {
            anchor: 0,
            focus: 0,
        },
    );
    let origin = editing_context(&s, 20, &binding()).unwrap();
    for text in ["a", "😀", "e\u{301}", "אב"] {
        accept(&mut q, &s, origin, text, &budget);
    }
    let mut combined = String::new();
    for (index, text) in ["a", "😀", "e\u{301}", "אב"].iter().enumerate() {
        let seq = index as u64 + 1;
        let prepared = q.start(seq, &s, &binding()).unwrap().unwrap();
        assert_eq!(prepared.text, *text);
        assert_eq!(
            prepared.context.selection.anchor,
            combined.chars().count() as u64
        );
        combined.push_str(text);
        let cursor = combined.chars().count() as u64;
        if index % 2 == 0 {
            apply(
                &mut s,
                &combined,
                TextRange {
                    anchor: cursor,
                    focus: cursor,
                },
            );
            assert!(!q.observe(&s, &binding()).unwrap());
            assert!(q.start(seq + 1, &s, &binding()).unwrap().is_none());
            q.result(seq, "Applied", "source-commit", Some(s.revision))
                .unwrap();
        } else {
            q.result(seq, "Applied", "source-commit", Some(s.revision + 1))
                .unwrap();
            assert!(!q.observe(&s, &binding()).unwrap());
            assert!(q.start(seq + 1, &s, &binding()).unwrap().is_none());
            apply(
                &mut s,
                &combined,
                TextRange {
                    anchor: cursor,
                    focus: cursor,
                },
            );
        }
        assert!(q.observe(&s, &binding()).unwrap());
        assert_eq!(q.context(), Some(origin));
    }
    assert_eq!(combined, "a😀e\u{301}אב");
    assert_eq!(q.pending_count(), 0);
    drop(q);
    assert_eq!(budget.used(), 0);
}
#[test]
fn same_content_replacement_confirms_unchanged_text_clock_and_collapsed_selection() {
    let budget = MemoryBudget::new(1 << 20);
    let mut q = TypingQueue::new(budget.clone()).unwrap();
    let mut s = fixture(
        "abc",
        TextRange {
            anchor: 0,
            focus: 3,
        },
    );
    let context = editing_context(&s, 20, &binding()).unwrap();
    accept(&mut q, &s, context, "abc", &budget);
    q.start(1, &s, &binding()).unwrap();
    apply(
        &mut s,
        "abc",
        TextRange {
            anchor: 3,
            focus: 3,
        },
    );
    q.result(1, "Applied", "source", Some(2)).unwrap();
    assert!(q.observe(&s, &binding()).unwrap());
    let Record::Text(t) = s.records.get(&40).unwrap() else {
        panic!()
    };
    assert_eq!(t.text_version, 1);
}
#[test]
fn unrelated_scene_revision_and_dispatched_result_cannot_advance_queue() {
    let budget = MemoryBudget::new(1 << 20);
    let mut q = TypingQueue::new(budget.clone()).unwrap();
    let mut s = fixture(
        "",
        TextRange {
            anchor: 0,
            focus: 0,
        },
    );
    let context = editing_context(&s, 20, &binding()).unwrap();
    accept(&mut q, &s, context, "x", &budget);
    q.start(1, &s, &binding()).unwrap();
    s.revision = 2;
    q.result(1, "Dispatched", "", None).unwrap();
    assert!(!q.observe(&s, &binding()).unwrap());
    q.result(99, "Applied", "unrelated", Some(2)).unwrap();
    assert!(!q.observe(&s, &binding()).unwrap());
    apply(
        &mut s,
        "x",
        TextRange {
            anchor: 1,
            focus: 1,
        },
    );
    q.result(1, "Applied", "own", Some(3)).unwrap();
    assert!(q.observe(&s, &binding()).unwrap());
}
#[test]
fn unexpected_text_selection_focus_identity_or_authority_cancels_explicitly() {
    for case in 0..9 {
        let budget = MemoryBudget::new(1 << 20);
        let mut q = TypingQueue::new(budget.clone()).unwrap();
        let mut s = fixture(
            "",
            TextRange {
                anchor: 0,
                focus: 0,
            },
        );
        let context = editing_context(&s, 20, &binding()).unwrap();
        accept(&mut q, &s, context, "x", &budget);
        accept(&mut q, &s, context, "y", &budget);
        q.start(1, &s, &binding()).unwrap();
        let mut b = binding();
        match case {
            0 => apply(
                &mut s,
                "other",
                TextRange {
                    anchor: 5,
                    focus: 5,
                },
            ),
            1 => apply(
                &mut s,
                "x",
                TextRange {
                    anchor: 0,
                    focus: 0,
                },
            ),
            2 => {
                let Record::Session(v) = s.records.get_mut(&50).unwrap() else {
                    panic!()
                };
                v.focus_generation += 1;
            }
            3 => {
                let Record::Session(v) = s.records.get_mut(&50).unwrap() else {
                    panic!()
                };
                v.focus = None;
            }
            4 => b.epoch = [5; 16],
            5 => b.view = [5; 16],
            6 => b.lease = [5; 16],
            7 => b.generation += 1,
            8 => {
                let Record::Semantic(n) = s.records.get_mut(&20).unwrap() else {
                    panic!()
                };
                n.text = None;
            }
            _ => unreachable!(),
        }
        assert_eq!(q.observe(&s, &b), Err(TypingError::Context), "case{case}");
        assert_eq!(q.cancel(), 2);
        drop(q);
        assert_eq!(budget.used(), 0);
    }
}
#[test]
fn rejection_missing_evidence_or_incorrect_revision_never_replays_pending_text() {
    for (status, source, revision) in [
        ("Rejected", "", None),
        ("OutcomeUnknown", "", None),
        ("Applied", "", Some(2)),
        ("Applied", "source", Some(1)),
    ] {
        let budget = MemoryBudget::new(1 << 20);
        let mut q = TypingQueue::new(budget.clone()).unwrap();
        let s = fixture(
            "",
            TextRange {
                anchor: 0,
                focus: 0,
            },
        );
        let context = editing_context(&s, 20, &binding()).unwrap();
        accept(&mut q, &s, context, "x", &budget);
        q.start(1, &s, &binding()).unwrap();
        assert_eq!(
            q.result(1, status, source, revision),
            Err(TypingError::Result)
        );
        assert_eq!(q.cancel(), 1);
    }
}

#[test]
fn omitted_installed_revision_needs_own_result_exact_context_and_later_source_state() {
    for state_first in [false, true] {
        let budget = MemoryBudget::new(1 << 20);
        let mut queue = TypingQueue::new(budget.clone()).unwrap();
        let mut state = fixture(
            "",
            TextRange {
                anchor: 0,
                focus: 0,
            },
        );
        let context = editing_context(&state, 20, &binding()).unwrap();
        accept(&mut queue, &state, context, "😀", &budget);
        accept(&mut queue, &state, context, "é", &budget);
        queue.start(1, &state, &binding()).unwrap();
        if state_first {
            apply(
                &mut state,
                "😀",
                TextRange {
                    anchor: 1,
                    focus: 1,
                },
            );
            assert!(!queue.observe(&state, &binding()).unwrap());
        }
        queue
            .result(99, "Applied", "foreign-native-cause", None)
            .unwrap();
        assert!(!queue.observe(&state, &binding()).unwrap());
        queue.result(1, "Dispatched", "", None).unwrap();
        assert!(!queue.observe(&state, &binding()).unwrap());
        queue
            .result(1, "Applied", "own-serialized-native-capture", None)
            .unwrap();
        if !state_first {
            // Causal text may follow an unrelated visual publication even
            // after the matching authenticated terminal result arrives.
            state.revision += 1;
            assert!(!queue.observe(&state, &binding()).unwrap());
            assert!(queue.start(2, &state, &binding()).unwrap().is_none());
            apply(
                &mut state,
                "😀",
                TextRange {
                    anchor: 1,
                    focus: 1,
                },
            );
        }
        // A later coalesced visual frame preserves the exact text clocks.
        state.revision += 1;
        assert!(queue.observe(&state, &binding()).unwrap());
        let second = queue.start(2, &state, &binding()).unwrap().unwrap();
        assert_eq!(
            second.context.selection,
            TextRange {
                anchor: 1,
                focus: 1
            }
        );
        assert_eq!(second.text, "é");
        drop(second);
        drop(queue);
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn coalesced_result_cannot_confirm_coincidental_text_after_context_aba_or_changed_cause() {
    for clock in 0..3 {
        let budget = MemoryBudget::new(1 << 20);
        let mut queue = TypingQueue::new(budget.clone()).unwrap();
        let mut state = fixture(
            "",
            TextRange {
                anchor: 0,
                focus: 0,
            },
        );
        let context = editing_context(&state, 20, &binding()).unwrap();
        accept(&mut queue, &state, context, "x", &budget);
        queue.start(1, &state, &binding()).unwrap();
        queue
            .result(1, "Applied", "own-native-capture", None)
            .unwrap();
        assert_eq!(
            queue.result(1, "Applied", "substituted-cause", None),
            Err(TypingError::Result)
        );
        apply(
            &mut state,
            "x",
            TextRange {
                anchor: 1,
                focus: 1,
            },
        );
        match clock {
            0 => {
                let Record::Text(text) = state.records.get_mut(&40).unwrap() else {
                    panic!()
                };
                text.version += 2;
            }
            1 => {
                let Record::Text(text) = state.records.get_mut(&40).unwrap() else {
                    panic!()
                };
                text.text_version += 2;
            }
            2 => {
                let Record::Session(session) = state.records.get_mut(&50).unwrap() else {
                    panic!()
                };
                session.focus_generation += 2;
            }
            _ => unreachable!(),
        }
        assert_eq!(queue.observe(&state, &binding()), Err(TypingError::Context));
        assert_eq!(queue.cancel(), 1);
        drop(queue);
        assert_eq!(budget.used(), 0);
    }
}
#[test]
fn source_clocks_cannot_skip_intervening_changes_even_with_expected_final_text() {
    for clock in 0..3 {
        let budget = MemoryBudget::new(1 << 20);
        let mut q = TypingQueue::new(budget.clone()).unwrap();
        let mut s = fixture(
            "",
            TextRange {
                anchor: 0,
                focus: 0,
            },
        );
        let context = editing_context(&s, 20, &binding()).unwrap();
        accept(&mut q, &s, context, "x", &budget);
        q.start(1, &s, &binding()).unwrap();
        apply(
            &mut s,
            "x",
            TextRange {
                anchor: 1,
                focus: 1,
            },
        );
        if clock < 2 {
            let Record::Text(t) = s.records.get_mut(&40).unwrap() else {
                panic!()
            };
            if clock == 0 {
                t.text_version += 1;
            } else {
                t.version += 1;
            }
        } else {
            let Record::Semantic(n) = s.records.get_mut(&20).unwrap() else {
                panic!()
            };
            n.version += 1;
        }
        q.result(1, "Applied", "own", Some(2)).unwrap();
        assert_eq!(q.observe(&s, &binding()), Err(TypingError::Context));
    }
}
#[test]
fn queue_bounds_and_memory_pressure_preserve_owned_inputs_until_explicit_cancel() {
    let budget = MemoryBudget::new(1 << 20);
    let mut q = TypingQueue::new(budget.clone()).unwrap();
    let s = fixture(
        "",
        TextRange {
            anchor: 0,
            focus: 0,
        },
    );
    let context = editing_context(&s, 20, &binding()).unwrap();
    for _ in 0..32 {
        accept(&mut q, &s, context, "x", &budget);
    }
    assert_eq!(
        q.accept(
            context,
            "x".into(),
            budget.reserve(2049).unwrap(),
            &s,
            &binding()
        ),
        Err(TypingError::Limit)
    );
    let pressure = budget.reserve(budget.limit() - budget.used()).unwrap();
    assert!(matches!(
        q.start(1, &s, &binding()),
        Err(TypingError::Memory)
    ));
    assert_eq!(q.pending_count(), 32);
    drop(pressure);
    assert!(q.start(1, &s, &binding()).unwrap().is_some());
    assert_eq!(q.cancel(), 32);
    drop(q);
    assert_eq!(budget.used(), 0);
}
