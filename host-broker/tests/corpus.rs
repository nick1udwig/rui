use prost::Message;
use rui_host_broker::*;
use rui_protocol_core::{
    commands::{CommandStatus, Rejection},
    model::{Materialization, Record, State, SurfaceRole, TypedValue},
    receiver::{ReceiveOutcome, ReceiverConfig, ViewReceiver},
    resources::{ResourceContext, ResourceScope},
    session::*,
    wire::proto as p,
};

const SECRET: &str = "CORPUS-PRIVATE-NETWORK-SENTINEL";
fn hello(id: u8, semantic: bool) -> ClientHello {
    let mut h = ClientHello {
        client_instance_id: [id; 16],
        supported_versions: [1].into_iter().collect(),
        supported_profiles: SessionConfig::default().supported_profiles,
        required_profiles: SessionConfig::default().required_profiles,
        receiver_limits: ReceiverLimits::default(),
        resume: None,
    };
    if semantic {
        h.supported_profiles.insert("semantics.only.v1".into());
        h.required_profiles.insert("semantics.only.v1".into());
    }
    h
}
fn auth(control: bool) -> VerifiedAuthorization {
    VerifiedAuthorization {
        principal: if control { "controller" } else { "observer" }.into(),
        policy_version: 1,
        permissions: Permissions {
            can_view: true,
            can_control: control,
            actions: if control {
                [
                    "Activate",
                    "Focus",
                    "ReplaceText",
                    "SetSelection",
                    "SetValue",
                    "ScrollTo",
                    READ_SECRET_PERMISSION,
                ]
                .into_iter()
                .map(str::to_owned)
                .collect()
            } else {
                Default::default()
            },
        },
    }
}
fn receiver(s: &NegotiatedSession) -> ViewReceiver {
    ViewReceiver::new(
        ResourceContext {
            scope: ResourceScope {
                session_id: s.session_id,
                epoch: s.epoch,
                principal_id: s.client.client_instance_id,
            },
            view_id: s.view_id,
        },
        s.limits.clone(),
        ReceiverConfig::default(),
    )
    .unwrap()
}
fn open(host: &mut ControlledHost, id: u8, control: bool) -> (NegotiatedSession, ViewReceiver) {
    let s = host
        .negotiate(auth(control), hello(id, control), 0)
        .unwrap();
    let mut r = receiver(&s);
    assert!(matches!(
        r.receive_envelope([1; 16], &host.checkpoint(&s.client, s.view_id).unwrap())
            .unwrap(),
        ReceiveOutcome::Applied { revision: 1 }
    ));
    host.acknowledge_applied(&s.client, s.view_id, 1).unwrap();
    if control {
        host.acknowledge_semantic_ready(&s.client, s.view_id, 1)
            .unwrap();
    }
    (s, r)
}
fn semantic(state: &State, id: u64) -> &rui_protocol_core::model::SemanticNode {
    let Record::Semantic(node) = &state.records[&id] else {
        panic!("semantic target")
    };
    node
}
fn command(
    s: &NegotiatedSession,
    l: &ControlLease,
    r: &ViewReceiver,
    seq: u64,
    id: u64,
    action: &str,
    args: p::semantic_command::Arguments,
) -> p::SemanticCommand {
    let state = r.state().unwrap();
    let n = semantic(state, id);
    let text = n.text.map(|reference| {
        let Record::Text(t) = &state.records[&reference.id] else {
            panic!("text")
        };
        t.text_version
    });
    let focus = state.records.values().find_map(|r| {
        if let Record::Session(s) = r {
            Some(s.focus_generation)
        } else {
            None
        }
    });
    let model = if id == LIST_ID || ITEM_IDS.contains(&id) {
        semantic(state, LIST_ID)
            .collection
            .as_ref()
            .map(|m| m.version)
    } else {
        None
    };
    p::SemanticCommand {
        epoch: s.epoch.to_vec(),
        view_id: s.view_id.to_vec(),
        client_instance_id: s.client.client_instance_id.to_vec(),
        command_seq: seq,
        control_lease_id: l.lease_id.to_vec(),
        control_generation: l.generation,
        target_id: id,
        action_id: action.into(),
        action_schema_version: 1,
        observed_view_revision: state.revision,
        preconditions: Some(p::Preconditions {
            target_version: Some(n.version),
            text_version: if matches!(action, "ReplaceText" | "SetSelection") {
                text
            } else {
                None
            },
            model_version: model,
            focus_generation: if matches!(action, "ReplaceText" | "SetSelection") {
                focus
            } else {
                None
            },
        }),
        arguments: Some(args),
    }
}
fn none() -> p::semantic_command::Arguments {
    p::semantic_command::Arguments::NoArguments(p::EmptyArguments {})
}
#[allow(clippy::too_many_arguments)] // Explicit source, authority and both replica scopes in this fixture helper.
fn apply(
    host: &mut ControlledHost,
    s: &NegotiatedSession,
    l: &ControlLease,
    c: &mut ViewReceiver,
    o: &NegotiatedSession,
    observer: &mut ViewReceiver,
    seq: u64,
    id: u64,
    action: &str,
    args: p::semantic_command::Arguments,
) {
    let raw = command(s, l, c, seq, id, action, args).encode_to_vec();
    let before = host.diagnostics().source_commit;
    let result = host.submit(&s.client, &raw, seq).unwrap();
    assert_eq!(
        result.status,
        CommandStatus::Applied,
        "{} target {}",
        action,
        id
    );
    assert_eq!(host.diagnostics().source_commit, before + 1);
    assert_eq!(host.submit(&s.client, &raw, seq).unwrap(), result);
    for (session, receiver) in [(s, c), (o, observer)] {
        let bytes = host
            .take_outbound(&session.client, session.view_id)
            .unwrap()
            .unwrap();
        if session.client == o.client {
            assert!(!bytes.windows(SECRET.len()).any(|s| s == SECRET.as_bytes()));
        }
        let revision = receiver.state().unwrap().revision + 1;
        assert!(
            matches!(receiver.receive_envelope((revision as u128).to_be_bytes(),&bytes).unwrap(),ReceiveOutcome::Applied{revision:r} if r==revision)
        );
        assert!(host
            .take_outbound(&session.client, session.view_id)
            .unwrap()
            .is_none());
        host.acknowledge_applied(&session.client, session.view_id, revision)
            .unwrap();
        if session.profiles.contains("semantics.only.v1") {
            host.acknowledge_semantic_ready(&session.client, session.view_id, revision)
                .unwrap();
        }
        let capture = host
            .source_capture(&session.client, session.view_id)
            .unwrap();
        assert_eq!(capture.source_commit(), before + 1);
        // A fresh source capture has no receiver replay-retirement history.
        // Compare the authoritative current graph without formatting content.
        assert_eq!(capture.state().revision, receiver.state().unwrap().revision);
        assert!(capture.state().records == receiver.state().unwrap().records);
        assert!(!format!("{capture:?}").contains(SECRET));
    }
}
fn host() -> ControlledHost {
    ControlledHost::new_corpus(
        HostConfig {
            max_text_bytes: 256,
            ..Default::default()
        },
        SECRET.into(),
    )
    .unwrap()
}

#[test]
fn corpus_unicode_selection_checkbox_collection_and_scrolling_replay_coherently() {
    let mut host = host();
    let (s, mut c) = open(&mut host, 3, true);
    let (o, mut observer) = open(&mut host, 4, false);
    let lease = host
        .acquire_control(&s.client, s.view_id, [5; 16], 0)
        .unwrap();
    apply(
        &mut host,
        &s,
        &lease,
        &mut c,
        &o,
        &mut observer,
        1,
        PUBLIC_TEXT_ID,
        "ReplaceText",
        p::semantic_command::Arguments::ReplaceText(p::TextArguments {
            start: 1,
            end: 2,
            text: "😀🦀".into(),
        }),
    );
    let Record::Text(t) = &c.state().unwrap().records[&PUBLIC_TEXT_MODEL_ID] else {
        panic!()
    };
    assert_eq!(t.content.as_deref(), Some("A😀🦀Ω אב"));
    assert_eq!(t.text_version, 2);
    apply(
        &mut host,
        &s,
        &lease,
        &mut c,
        &o,
        &mut observer,
        2,
        PUBLIC_TEXT_ID,
        "SetSelection",
        p::semantic_command::Arguments::SetSelection(p::SelectionArguments {
            anchor: 4,
            focus: 1,
        }),
    );
    let Record::Text(t) = &c.state().unwrap().records[&PUBLIC_TEXT_MODEL_ID] else {
        panic!()
    };
    assert_eq!(t.text_version, 2);
    assert_eq!(t.selection.unwrap().anchor, 4);
    apply(
        &mut host,
        &s,
        &lease,
        &mut c,
        &o,
        &mut observer,
        3,
        CHECKBOX_ID,
        "SetValue",
        p::semantic_command::Arguments::SetValue(p::ValueArguments {
            value: Some(p::value_arguments::Value::Checked(true)),
        }),
    );
    assert_eq!(
        semantic(c.state().unwrap(), CHECKBOX_ID).value,
        Some(TypedValue::Boolean(true))
    );
    apply(
        &mut host,
        &s,
        &lease,
        &mut c,
        &o,
        &mut observer,
        4,
        REORDER_ID,
        "Activate",
        none(),
    );
    let model = semantic(c.state().unwrap(), LIST_ID)
        .collection
        .as_ref()
        .unwrap();
    assert_eq!(model.total_count, Some(10));
    assert_eq!(model.materialization, Materialization::Partial);
    assert_eq!(
        model.items.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![71, 72, 70]
    );
    assert_eq!(model.version, 2);
    apply(
        &mut host,
        &s,
        &lease,
        &mut c,
        &o,
        &mut observer,
        5,
        ITEM_IDS[2],
        "Activate",
        none(),
    );
    let model = semantic(c.state().unwrap(), LIST_ID)
        .collection
        .as_ref()
        .unwrap();
    assert_eq!(model.selected_items[0].id, 72);
    assert_eq!(model.version, 3);
    apply(
        &mut host,
        &s,
        &lease,
        &mut c,
        &o,
        &mut observer,
        6,
        LIST_ID,
        "ScrollTo",
        p::semantic_command::Arguments::ScrollTo(p::ScrollArguments { x: 0.0, y: 60.0 }),
    );
    for r in observer.state().unwrap().records.values() {
        match r {
            Record::Scene(n) if (120..123).contains(&n.id) => assert_eq!(n.clip.unwrap().y, 110.0),
            Record::Binding(b) if (220..223).contains(&b.id) => {
                if let Some(rect) = b.interaction_region {
                    assert!(rect.y >= 110.0 && rect.y + rect.height <= 220.0);
                }
            }
            _ => {}
        }
    }
}

#[test]
fn corpus_transient_ids_are_fresh_modality_blocks_main_and_focus_restores() {
    let mut host = host();
    let (s, mut c) = open(&mut host, 3, true);
    let (o, mut observer) = open(&mut host, 4, false);
    let lease = host
        .acquire_control(&s.client, s.view_id, [5; 16], 0)
        .unwrap();
    apply(
        &mut host,
        &s,
        &lease,
        &mut c,
        &o,
        &mut observer,
        1,
        POPUP_TOGGLE_ID,
        "Activate",
        none(),
    );
    let popup = observer
        .state()
        .unwrap()
        .records
        .values()
        .find_map(|r| {
            if let Record::Surface(s) = r {
                (s.role == SurfaceRole::Popup).then_some(s.id)
            } else {
                None
            }
        })
        .unwrap();
    apply(
        &mut host,
        &s,
        &lease,
        &mut c,
        &o,
        &mut observer,
        2,
        POPUP_TOGGLE_ID,
        "Activate",
        none(),
    );
    assert!(observer.state().unwrap().retired.contains(&popup));
    apply(
        &mut host,
        &s,
        &lease,
        &mut c,
        &o,
        &mut observer,
        3,
        POPUP_TOGGLE_ID,
        "Activate",
        none(),
    );
    assert!(!observer.state().unwrap().records.contains_key(&popup));
    apply(
        &mut host,
        &s,
        &lease,
        &mut c,
        &o,
        &mut observer,
        4,
        MODAL_TOGGLE_ID,
        "Activate",
        none(),
    );
    let modal = observer
        .state()
        .unwrap()
        .records
        .values()
        .find_map(|r| {
            if let Record::Surface(s) = r {
                s.modal_for.map(|_| s.id)
            } else {
                None
            }
        })
        .unwrap();
    let before = host.diagnostics().source_commit;
    let raw = command(&s, &lease, &c, 5, COUNTER_ID, "Activate", none()).encode_to_vec();
    assert_eq!(
        host.submit(&s.client, &raw, 5).unwrap().rejection,
        Some(Rejection::DisabledTarget)
    );
    assert_eq!(host.diagnostics().source_commit, before);
    apply(
        &mut host,
        &s,
        &lease,
        &mut c,
        &o,
        &mut observer,
        6,
        modal + 2,
        "Activate",
        none(),
    );
    let focus = c.state().unwrap().records.values().find_map(|r| {
        if let Record::Session(s) = r {
            s.focus.map(|r| r.id)
        } else {
            None
        }
    });
    assert_eq!(focus, Some(PUBLIC_TEXT_ID));
    apply(
        &mut host,
        &s,
        &lease,
        &mut c,
        &o,
        &mut observer,
        7,
        TOOLTIP_TOGGLE_ID,
        "Activate",
        none(),
    );
    assert!(observer.state().unwrap().records.values().any(|r|matches!(r,Record::Surface(s) if s.role==SurfaceRole::Tooltip&&s.input_region.is_empty())));
}

#[test]
fn source_captures_require_published_context_and_remain_budgeted_without_resetting_history() {
    let mut host = host();
    let s = host.negotiate(auth(true), hello(3, true), 0).unwrap();
    assert!(matches!(
        host.source_capture(&s.client, s.view_id),
        Err(HostError::CheckpointRequired)
    ));
    let snapshot = host.checkpoint(&s.client, s.view_id).unwrap();
    drop(snapshot);
    let base = host.retained_memory_bytes(&s.client, s.view_id).unwrap();
    let capture = host.source_capture(&s.client, s.view_id).unwrap();
    assert!(host.retained_memory_bytes(&s.client, s.view_id).unwrap() > base);
    drop(capture);
    assert_eq!(
        host.retained_memory_bytes(&s.client, s.view_id).unwrap(),
        base
    );
    let desc = host.view_description(&s.client, s.view_id).unwrap();
    assert_eq!(desc.coverage, "semantic-only");
    assert_eq!((desc.width, desc.height), (920.0, 480.0));
    host.disconnect(&s.client, 1).unwrap();
    host.set_counter_enabled(false, 2).unwrap();
    let mut h = hello(3, true);
    h.resume = Some(ResumeRequest {
        epoch: s.epoch,
        view_id: s.view_id,
        acknowledged_revision: 1,
    });
    let fresh = host.negotiate(auth(true), h, 3).unwrap();
    assert!(matches!(fresh.resume, ResumeDisposition::ResetRequired(_)));
    assert!(matches!(
        host.source_capture(&s.client, s.view_id),
        Err(HostError::CheckpointRequired)
    ));
}

#[test]
fn corpus_stale_text_focus_model_observer_and_old_lease_reject_before_source_mutation() {
    let mut host = host();
    let (s, mut c) = open(&mut host, 3, true);
    let (o, mut observer) = open(&mut host, 4, false);
    let lease = host
        .acquire_control(&s.client, s.view_id, [5; 16], 0)
        .unwrap();
    let text_args = || {
        p::semantic_command::Arguments::ReplaceText(p::TextArguments {
            start: 0,
            end: 1,
            text: "B".into(),
        })
    };
    apply(
        &mut host,
        &s,
        &lease,
        &mut c,
        &o,
        &mut observer,
        1,
        PUBLIC_TEXT_ID,
        "ReplaceText",
        text_args(),
    );
    let mut stale = command(
        &s,
        &lease,
        &c,
        2,
        PUBLIC_TEXT_ID,
        "ReplaceText",
        text_args(),
    );
    stale.preconditions.as_mut().unwrap().text_version = Some(1);
    let before = host.diagnostics().source_commit;
    assert_eq!(
        host.submit(&s.client, &stale.encode_to_vec(), 2)
            .unwrap()
            .rejection,
        Some(Rejection::StaleText)
    );
    apply(
        &mut host,
        &s,
        &lease,
        &mut c,
        &o,
        &mut observer,
        3,
        SECRET_ID,
        "Focus",
        none(),
    );
    let stale = command(
        &s,
        &lease,
        &c,
        4,
        PUBLIC_TEXT_ID,
        "ReplaceText",
        text_args(),
    );
    assert_eq!(
        host.submit(&s.client, &stale.encode_to_vec(), 4)
            .unwrap()
            .rejection,
        Some(Rejection::StaleFocus)
    );
    apply(
        &mut host,
        &s,
        &lease,
        &mut c,
        &o,
        &mut observer,
        5,
        REORDER_ID,
        "Activate",
        none(),
    );
    let mut stale = command(&s, &lease, &c, 6, ITEM_IDS[0], "Activate", none());
    stale.preconditions.as_mut().unwrap().model_version = Some(1);
    assert_eq!(
        host.submit(&s.client, &stale.encode_to_vec(), 6)
            .unwrap()
            .rejection,
        Some(Rejection::StaleModel)
    );
    let outsider = command(
        &o,
        &lease,
        &observer,
        1,
        CHECKBOX_ID,
        "SetValue",
        p::semantic_command::Arguments::SetValue(p::ValueArguments {
            value: Some(p::value_arguments::Value::Checked(true)),
        }),
    );
    assert_eq!(
        host.submit(&o.client, &outsider.encode_to_vec(), 6)
            .unwrap()
            .rejection,
        Some(Rejection::Unauthorized)
    );
    host.local_activity(7);
    let stale = command(&s, &lease, &c, 7, COUNTER_ID, "Activate", none());
    assert_eq!(
        host.submit(&s.client, &stale.encode_to_vec(), 7)
            .unwrap()
            .rejection,
        Some(Rejection::StaleControl)
    );
    assert_eq!(host.diagnostics().source_commit, before + 2);
    assert!(!format!("{:?}", host.diagnostics()).contains(SECRET));
}

#[test]
fn captured_handles_and_scratch_share_ceiling_failed_reads_preserve_queued_history() {
    let mut config = HostConfig {
        max_text_bytes: 256,
        ..Default::default()
    };
    config.session.receiver_limits.view_memory_bytes = 2 * 1024 * 1024;
    let mut host = ControlledHost::new_corpus(config, SECRET.into()).unwrap();
    let (s, mut c) = open(&mut host, 3, true);
    let (o, mut observer) = open(&mut host, 4, false);
    let lease = host
        .acquire_control(&s.client, s.view_id, [5; 16], 0)
        .unwrap();
    apply(
        &mut host,
        &s,
        &lease,
        &mut c,
        &o,
        &mut observer,
        1,
        COUNTER_ID,
        "Activate",
        none(),
    );
    // Leave a real dependent update queued and unacknowledged while reads fail.
    host.set_counter_enabled(false, 2).unwrap();
    let first = host.source_capture(&s.client, s.view_id).unwrap();
    let second = host.source_capture(&s.client, s.view_id).unwrap();
    let used = host.retained_memory_bytes(&s.client, s.view_id).unwrap();
    assert!(used <= s.limits.view_memory_bytes);
    let status = host.view_status(&s.client, s.view_id).unwrap();
    assert!(matches!(
        host.snapshot(&s.client, s.view_id),
        Err(HostError::Session(SessionError::Oversized))
    ));
    assert!(matches!(
        host.source_capture(&s.client, s.view_id),
        Err(HostError::Session(SessionError::Oversized))
    ));
    assert_eq!(
        host.retained_memory_bytes(&s.client, s.view_id).unwrap(),
        used
    );
    assert_eq!(host.view_status(&s.client, s.view_id).unwrap(), status);
    drop(first);
    drop(second);
    assert!(host.snapshot(&s.client, s.view_id).is_ok());
    host.disconnect(&s.client, 2).unwrap();
    let mut h = hello(3, true);
    h.receiver_limits.view_memory_bytes = s.limits.view_memory_bytes;
    h.resume = Some(ResumeRequest {
        epoch: s.epoch,
        view_id: s.view_id,
        acknowledged_revision: 2,
    });
    let resumed = host.negotiate(auth(true), h, 3).unwrap();
    assert!(
        matches!(resumed.resume,ResumeDisposition::Accepted{from_revision:2,through_revision:3,ref transactions,..} if transactions.len()==1)
    );
}

#[test]
fn corpus_private_edits_never_echo_into_observer_capture_history_or_resume() {
    let mut host = host();
    let (s, mut c) = open(&mut host, 3, true);
    let (o, mut observer) = open(&mut host, 4, false);
    let lease = host
        .acquire_control(&s.client, s.view_id, [5; 16], 0)
        .unwrap();
    let replacement = "NEW-CORPUS-SECRET-SENTINEL";
    for (sequence, target, action, args) in [
        (1, SECRET_ID, "Focus", none()),
        (
            2,
            SECRET_ID,
            "ReplaceText",
            p::semantic_command::Arguments::ReplaceText(p::TextArguments {
                start: 0,
                end: SECRET.chars().count() as u64,
                text: replacement.into(),
            }),
        ),
    ] {
        let raw = command(&s, &lease, &c, sequence, target, action, args).encode_to_vec();
        assert_eq!(
            host.submit(&s.client, &raw, sequence).unwrap().status,
            CommandStatus::Applied
        );
        for (session, receiver) in [(&s, &mut c), (&o, &mut observer)] {
            let bytes = host
                .take_outbound(&session.client, session.view_id)
                .unwrap()
                .unwrap();
            if session.client == o.client {
                for secret in [SECRET, replacement] {
                    assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_bytes()));
                }
            }
            receiver
                .receive_envelope((sequence as u128 + 1).to_be_bytes(), &bytes)
                .unwrap();
            if session.client == s.client {
                host.acknowledge_applied(&s.client, s.view_id, sequence + 1)
                    .unwrap();
                host.acknowledge_semantic_ready(&s.client, s.view_id, sequence + 1)
                    .unwrap();
            }
        }
    }
    let capture = host.source_capture(&o.client, o.view_id).unwrap();
    let Record::Text(private) = &capture.state().records[&SECRET_TEXT_ID] else {
        panic!()
    };
    assert!(private.content.is_none());
    let Record::Scene(private_scene) = &capture.state().records[&SECRET_SCENE_ID] else {
        panic!()
    };
    assert_eq!(private_scene.version, 1);
    assert!(!format!("{capture:?}").contains(replacement));
    drop(capture);
    host.disconnect(&o.client, 3).unwrap();
    let mut h = hello(4, false);
    h.resume = Some(ResumeRequest {
        epoch: o.epoch,
        view_id: o.view_id,
        acknowledged_revision: 1,
    });
    let resumed = host.negotiate(auth(false), h, 4).unwrap();
    let ResumeDisposition::Accepted { transactions, .. } = resumed.resume else {
        panic!("retained chain required")
    };
    assert_eq!(transactions.len(), 2);
    let snapshot = host.snapshot(&o.client, o.view_id).unwrap();
    for bytes in transactions
        .iter()
        .map(|p| p.as_ref())
        .chain([snapshot.as_ref()])
    {
        for secret in [SECRET, replacement] {
            assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_bytes()));
        }
    }
}

#[test]
fn source_commit_survives_one_views_scratch_failure_without_inventing_its_revision() {
    let mut config = HostConfig {
        max_text_bytes: 256,
        ..Default::default()
    };
    config.session.receiver_limits.view_memory_bytes = 2 * 1024 * 1024;
    let mut host = ControlledHost::new_corpus(config, SECRET.into()).unwrap();
    let (s, mut c) = open(&mut host, 3, true);
    let (o, mut observer) = open(&mut host, 4, false);
    let lease = host
        .acquire_control(&s.client, s.view_id, [5; 16], 0)
        .unwrap();
    apply(
        &mut host,
        &s,
        &lease,
        &mut c,
        &o,
        &mut observer,
        1,
        COUNTER_ID,
        "Activate",
        none(),
    );
    let _one = host.source_capture(&s.client, s.view_id).unwrap();
    let _two = host.source_capture(&s.client, s.view_id).unwrap();
    let raw = command(&s, &lease, &c, 2, COUNTER_ID, "Activate", none()).encode_to_vec();
    let result = host.submit(&s.client, &raw, 2).unwrap();
    assert_eq!(result.status, CommandStatus::Applied);
    assert!(result.evidence.unwrap().resulting_view_revision().is_none());
    assert_eq!(host.diagnostics().counter, 2);
    assert_eq!(host.diagnostics().consumed_commands, 2);
    assert_eq!(
        host.view_status(&s.client, s.view_id).unwrap().closed,
        Some(PublicationFailure::SlowConsumer)
    );
    let bytes = host.take_outbound(&o.client, o.view_id).unwrap().unwrap();
    assert!(matches!(
        observer.receive_envelope([3; 16], &bytes).unwrap(),
        ReceiveOutcome::Applied { revision: 3 }
    ));
    assert!(host.submit(&s.client, &raw, 3).is_err());
    assert_eq!(host.diagnostics().counter, 2);
}
