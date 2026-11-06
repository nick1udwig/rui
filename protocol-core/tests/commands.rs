use prost::Message;
use rui_protocol_core::commands::*;
use rui_protocol_core::wire::proto;
use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};

fn client() -> AuthenticatedClient {
    AuthenticatedClient {
        principal: "alice".into(),
        client_instance_id: [3; 16],
    }
}
fn wire(sequence: u64) -> proto::SemanticCommand {
    proto::SemanticCommand {
        epoch: vec![1; 16],
        view_id: vec![2; 16],
        client_instance_id: vec![3; 16],
        command_seq: sequence,
        control_lease_id: vec![4; 16],
        control_generation: 1,
        target_id: 10,
        action_id: "Activate".into(),
        action_schema_version: 1,
        observed_view_revision: 5,
        preconditions: Some(proto::Preconditions {
            target_version: Some(5),
            text_version: None,
            model_version: None,
            focus_generation: None,
        }),
        arguments: Some(proto::semantic_command::Arguments::NoArguments(
            proto::EmptyArguments {},
        )),
    }
}
fn ledger() -> CommandLedger {
    CommandLedger::new([1; 16], LedgerLimits::default()).unwrap()
}
fn target() -> TargetState {
    TargetState {
        enabled: true,
        target_version: 5,
        text_version: Some(6),
        model_version: Some(4),
        focus_generation: Some(7),
        text_scalar_count: Some(3),
        value_schema: Some(ValueSchema::Boolean),
        actions: [
            "Activate",
            "SetValue",
            "ReplaceText",
            "SetSelection",
            "ScrollTo",
            "Focus",
        ]
        .into_iter()
        .map(|a| (a.into(), 1))
        .collect(),
    }
}
fn authority() -> SemanticAuthority {
    SemanticAuthority {
        view_id: [2; 16],
        current_view_revision: 50,
        allowed_actions: [
            "Activate",
            "SetValue",
            "ReplaceText",
            "SetSelection",
            "ScrollTo",
            "Focus",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        targets: [(10, target())].into_iter().collect(),
    }
}

#[test]
fn cmd_001_retry_has_one_effect_and_returns_exact_recorded_result() {
    let mut ledger = ledger();
    let count = Cell::new(0);
    let raw = wire(1).encode_to_vec();
    let result = ledger
        .submit(
            &client(),
            &raw,
            0,
            |_| Ok(()),
            |_| {
                count.set(count.get() + 1);
                Ok(DispatchOutcome::Applied(
                    SourceEvidence::new("source-commit-1".into(), Some(6)).unwrap(),
                ))
            },
        )
        .unwrap();
    let retry = ledger
        .submit(
            &client(),
            &raw,
            1,
            |_| panic!("retry must not revalidate"),
            |_| panic!("retry must not dispatch"),
        )
        .unwrap();
    assert_eq!(count.get(), 1);
    assert_eq!(result, retry);
    assert_eq!(ledger.high_water(&client()), 1);
}
#[test]
fn cmd_002_changed_exact_bytes_reject_even_with_same_decoded_semantics() {
    let mut ledger = ledger();
    let raw = wire(1).encode_to_vec();
    ledger
        .submit(
            &client(),
            &raw,
            0,
            |_| Ok(()),
            |_| Ok(DispatchOutcome::Dispatched),
        )
        .unwrap();
    let mut replacement = wire(1);
    replacement.target_id = 11;
    assert_eq!(
        ledger.submit(
            &client(),
            &replacement.encode_to_vec(),
            1,
            |_| Ok(()),
            |_| panic!()
        ),
        Err(LedgerError::ChangedBytes)
    );
    // Unknown optional fields do not change behavior but are still different bytes
    // and therefore cannot be used to retransmit an already-consumed key.
    let mut optional = raw;
    optional.extend_from_slice(&[0xb8, 0x3e, 0x01]);
    assert_eq!(
        ledger.submit(&client(), &optional, 2, |_| Ok(()), |_| panic!()),
        Err(LedgerError::ChangedBytes)
    );
}
#[test]
fn cmd_003_gap_reject_and_well_formed_rejection_consumes_sequence() {
    let mut ledger = ledger();
    for seq in 1..=4 {
        let result = ledger
            .submit(
                &client(),
                &wire(seq).encode_to_vec(),
                0,
                |_| Err(Rejection::DisabledTarget),
                |_| panic!(),
            )
            .unwrap();
        assert_eq!(result.status, CommandStatus::Rejected);
    }
    assert_eq!(
        ledger.submit(
            &client(),
            &wire(6).encode_to_vec(),
            0,
            |_| Ok(()),
            |_| panic!()
        ),
        Err(LedgerError::SequenceGap)
    );
    assert_eq!(ledger.high_water(&client()), 4);
    ledger
        .submit(
            &client(),
            &wire(5).encode_to_vec(),
            0,
            |_| Ok(()),
            |_| Ok(DispatchOutcome::Dispatched),
        )
        .unwrap();
}
#[test]
fn cmd_004_expired_outcomes_never_redispatch_or_allow_changed_bytes() {
    let limits = LedgerLimits {
        outcome_ttl_ms: 10,
        ..LedgerLimits::default()
    };
    let mut ledger = CommandLedger::new([1; 16], limits).unwrap();
    let raw = wire(1).encode_to_vec();
    ledger
        .submit(
            &client(),
            &raw,
            100,
            |_| Ok(()),
            |_| Ok(DispatchOutcome::Dispatched),
        )
        .unwrap();
    assert_eq!(
        ledger
            .submit(&client(), &raw, 110, |_| panic!(), |_| panic!())
            .unwrap()
            .status,
        CommandStatus::OutcomeExpired
    );
    assert_eq!(
        ledger.get_result(&client(), [1; 16], 1, 1).unwrap().status,
        CommandStatus::OutcomeExpired
    );
    let mut changed = wire(1);
    changed.target_id = 11;
    assert_eq!(
        ledger.submit(
            &client(),
            &changed.encode_to_vec(),
            1,
            |_| Ok(()),
            |_| panic!()
        ),
        Err(LedgerError::ChangedBytes)
    );
    assert_eq!(ledger.high_water(&client()), 1);
}
#[test]
fn cmd_005_reject_stale_text_without_overwriting_host_version() {
    let mut ledger = ledger();
    let mut wire = wire(1);
    wire.action_id = "ReplaceText".into();
    wire.preconditions.as_mut().unwrap().text_version = Some(5);
    wire.arguments = Some(proto::semantic_command::Arguments::ReplaceText(
        proto::TextArguments {
            start: 0,
            end: 3,
            text: "new".into(),
        },
    ));
    let result = ledger
        .submit(
            &client(),
            &wire.encode_to_vec(),
            0,
            |c| authority().validate(c),
            |_| panic!(),
        )
        .unwrap();
    assert_eq!(result.rejection, Some(Rejection::StaleText));
    assert_eq!(ledger.high_water(&client()), 1);
}
#[test]
fn cmd_006_unrelated_animation_does_not_invalidate_target() {
    let command = decode_command(&wire(1).encode_to_vec()).unwrap();
    assert!(command.observed_view_revision < authority().current_view_revision);
    assert_eq!(authority().validate(&command), Ok(()));
    let mut authority = authority();
    authority.targets.get_mut(&10).unwrap().target_version = 6;
    assert_eq!(authority.validate(&command), Err(Rejection::StaleTarget));
}
#[test]
fn cmd_007_deleted_disabled_and_unsupported_targets_reject() {
    let command = decode_command(&wire(1).encode_to_vec()).unwrap();
    let mut authority = authority();
    authority.targets.remove(&10);
    authority.targets.insert(11, target());
    assert_eq!(authority.validate(&command), Err(Rejection::MissingTarget));
    authority.targets.insert(10, target());
    authority.targets.get_mut(&10).unwrap().enabled = false;
    assert_eq!(authority.validate(&command), Err(Rejection::DisabledTarget));
    authority.targets.get_mut(&10).unwrap().enabled = true;
    authority
        .targets
        .get_mut(&10)
        .unwrap()
        .actions
        .remove("Activate");
    assert_eq!(
        authority.validate(&command),
        Err(Rejection::UnsupportedAction)
    );
}
#[test]
fn cmd_008_ambiguous_dispatch_is_unknown_and_never_automatically_repeated() {
    let mut ledger = ledger();
    let raw = wire(1).encode_to_vec();
    let result = ledger
        .submit(
            &client(),
            &raw,
            0,
            |_| Ok(()),
            |_| Err(DispatchFailure::OutcomeUnknown),
        )
        .unwrap();
    assert_eq!(result.status, CommandStatus::OutcomeUnknown);
    assert!(result.evidence.is_none());
    assert_eq!(
        ledger
            .submit(&client(), &raw, 1, |_| panic!(), |_| panic!())
            .unwrap(),
        result
    );
    let raw2 = wire(2).encode_to_vec();
    assert_eq!(
        ledger
            .submit(
                &client(),
                &raw2,
                1,
                |_| Ok(()),
                |_| Ok(DispatchOutcome::Dispatched)
            )
            .unwrap()
            .status,
        CommandStatus::Dispatched
    );
}
#[test]
fn reservation_survives_adapter_panic_after_external_effect() {
    let mut ledger = ledger();
    let count = Cell::new(0);
    let raw = wire(1).encode_to_vec();
    let panic = catch_unwind(AssertUnwindSafe(|| {
        ledger.submit(
            &client(),
            &raw,
            0,
            |_| Ok(()),
            |_| {
                count.set(count.get() + 1);
                panic!("connection lost after side effect");
            },
        )
    }));
    assert!(panic.is_err());
    assert_eq!(count.get(), 1);
    assert_eq!(
        ledger
            .submit(&client(), &raw, 1, |_| panic!(), |_| panic!())
            .unwrap()
            .status,
        CommandStatus::OutcomeUnknown
    );
    assert_eq!(count.get(), 1);
}
#[test]
fn cmd_009_applied_requires_evidence_and_can_have_no_visible_change() {
    let mut ledger = ledger();
    assert_eq!(
        SourceEvidence::new("".into(), None),
        Err(LedgerError::InvalidEvidence)
    );
    assert_eq!(
        SourceEvidence::new("commit".into(), Some(0)),
        Err(LedgerError::InvalidEvidence)
    );
    for (seq, revision) in [(1, Some(8)), (2, None)] {
        let evidence = SourceEvidence::new(format!("commit-{seq}"), revision).unwrap();
        let result = ledger
            .submit(
                &client(),
                &wire(seq).encode_to_vec(),
                0,
                |_| Ok(()),
                |_| Ok(DispatchOutcome::Applied(evidence.clone())),
            )
            .unwrap();
        assert_eq!(result.status, CommandStatus::Applied);
        assert_eq!(result.evidence, Some(evidence));
    }
}
#[test]
fn outcome_retention_uses_global_receipt_order_across_clients_and_expiry() {
    assert!(matches!(
        CommandLedger::new(
            [1; 16],
            LedgerLimits {
                max_outcomes: 0,
                ..LedgerLimits::default()
            }
        ),
        Err(LedgerError::InvalidLimits)
    ));
    let clients = ["charlie", "alice", "bob"].map(|principal| AuthenticatedClient {
        principal: principal.into(),
        ..client()
    });
    let arrivals = [
        (0, 1, 0),
        (1, 1, 4),
        (2, 1, 4),
        (0, 2, 8),
        (2, 2, 11),
        (1, 2, 11),
        (0, 3, 12),
        (1, 3, 30),
    ];
    for maximum in [1, 3, arrivals.len()] {
        let mut ledger = CommandLedger::new(
            [1; 16],
            LedgerLimits {
                max_outcomes: maximum,
                outcome_ttl_ms: 10,
                ..LedgerLimits::default()
            },
        )
        .unwrap();
        let mut retained = std::collections::VecDeque::new();
        for (index, &(producer, sequence, now)) in arrivals.iter().enumerate() {
            ledger
                .submit(
                    &clients[producer],
                    &wire(sequence).encode_to_vec(),
                    now,
                    |_| Ok(()),
                    |_| Ok(DispatchOutcome::Dispatched),
                )
                .unwrap();
            retained.retain(|&old: &usize| arrivals[old].2 + 10 > now);
            retained.push_back(index);
            if retained.len() > maximum {
                retained.pop_front();
            }
            for (old, &(producer, sequence, _)) in arrivals[..=index].iter().enumerate() {
                let expected = if retained.contains(&old) {
                    CommandStatus::Dispatched
                } else {
                    CommandStatus::OutcomeExpired
                };
                assert_eq!(
                    ledger
                        .get_result(&clients[producer], [1; 16], sequence, now)
                        .unwrap()
                        .status,
                    expected,
                    "maximum={maximum}, arrival={index}, command={old}"
                );
                assert_eq!(
                    ledger
                        .submit(
                            &clients[producer],
                            &wire(sequence).encode_to_vec(),
                            now,
                            |_| panic!("retry must not validate"),
                            |_| panic!("retry must not dispatch"),
                        )
                        .unwrap()
                        .status,
                    expected
                );
            }
        }
    }
}

#[test]
fn retention_is_bounded_without_forgetting_epoch_high_water() {
    let limits = LedgerLimits {
        max_clients: 1,
        max_commands: 2,
        max_outcomes: 1,
        ..LedgerLimits::default()
    };
    let mut ledger = CommandLedger::new([1; 16], limits).unwrap();
    for seq in 1..=2 {
        ledger
            .submit(
                &client(),
                &wire(seq).encode_to_vec(),
                0,
                |_| Ok(()),
                |_| Ok(DispatchOutcome::Dispatched),
            )
            .unwrap();
    }
    assert_eq!(
        ledger.get_result(&client(), [1; 16], 1, 0).unwrap().status,
        CommandStatus::OutcomeExpired
    );
    assert_eq!(
        ledger.submit(
            &client(),
            &wire(3).encode_to_vec(),
            0,
            |_| Ok(()),
            |_| panic!()
        ),
        Err(LedgerError::RetentionExhausted)
    );
    let other = AuthenticatedClient {
        principal: "bob".into(),
        client_instance_id: [8; 16],
    };
    let mut other_wire = wire(1);
    other_wire.client_instance_id = vec![8; 16];
    assert_eq!(
        ledger.submit(
            &other,
            &other_wire.encode_to_vec(),
            0,
            |_| Ok(()),
            |_| panic!()
        ),
        Err(LedgerError::RetentionExhausted)
    );
    assert_eq!(ledger.high_water(&client()), 2);
    assert_eq!(ledger.producer_count(), 1);
    assert_eq!(ledger.command_count(), 2);
    assert_eq!(
        ledger
            .submit(
                &client(),
                &wire(1).encode_to_vec(),
                0,
                |_| panic!(),
                |_| panic!()
            )
            .unwrap()
            .status,
        CommandStatus::OutcomeExpired
    );
}
#[test]
fn malformed_old_epoch_and_registered_client_mismatch_do_not_consume() {
    let mut ledger = ledger();
    assert_eq!(
        ledger.submit(&client(), &[0xff], 0, |_| Ok(()), |_| panic!()),
        Err(LedgerError::Malformed)
    );
    let mut old = wire(1);
    old.epoch = vec![0; 16];
    assert_eq!(
        ledger.submit(&client(), &old.encode_to_vec(), 0, |_| Ok(()), |_| panic!()),
        Err(LedgerError::WrongEpoch)
    );
    let mut forged = wire(1);
    forged.client_instance_id = vec![9; 16];
    assert_eq!(
        ledger.submit(
            &client(),
            &forged.encode_to_vec(),
            0,
            |_| Ok(()),
            |_| panic!()
        ),
        Err(LedgerError::WrongClient)
    );
    assert_eq!(ledger.high_water(&client()), 0);
    assert_eq!(
        decode_command(&vec![0; 1024 * 1024 + 1]),
        Err(LedgerError::Oversized)
    );
}
#[test]
fn typed_arguments_scalar_ranges_and_relevant_context_are_checked() {
    let mut command = decode_command(&wire(1).encode_to_vec()).unwrap();
    command.action_id = "SetValue".into();
    command.arguments = Arguments::Number(1.0);
    assert_eq!(
        authority().validate(&command),
        Err(Rejection::InvalidArguments)
    );
    command.arguments = Arguments::Checked(true);
    assert_eq!(authority().validate(&command), Ok(()));
    command.action_id = "ReplaceText".into();
    command.arguments = Arguments::ReplaceText {
        start: 0,
        end: 4,
        text: "😀".into(),
    };
    command.preconditions.text_version = Some(6);
    assert_eq!(
        authority().validate(&command),
        Err(Rejection::InvalidArguments)
    );
    command.arguments = Arguments::ReplaceText {
        start: 0,
        end: 3,
        text: "😀".into(),
    };
    assert_eq!(authority().validate(&command), Ok(()));
    command.preconditions.focus_generation = Some(6);
    assert_eq!(authority().validate(&command), Err(Rejection::StaleFocus));
    command.preconditions.focus_generation = Some(7);
    command.preconditions.model_version = Some(3);
    assert_eq!(authority().validate(&command), Err(Rejection::StaleModel));
}

fn controlled() -> (
    rui_protocol_core::session::SessionBroker,
    rui_protocol_core::session::NegotiatedSession,
    rui_protocol_core::session::ControlLease,
) {
    use rui_protocol_core::session::*;
    let mut sessions = SessionBroker::new([1; 16], [9; 16], SessionConfig::default()).unwrap();
    let session = sessions
        .negotiate(
            Some(VerifiedAuthorization {
                principal: "alice".into(),
                policy_version: 1,
                permissions: Permissions {
                    can_view: true,
                    can_control: true,
                    actions: [
                        "Activate",
                        "RawInputBatch",
                        "TextComposition",
                        "ResizeRequest",
                    ]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                },
            }),
            ClientHello {
                client_instance_id: [3; 16],
                supported_versions: [1].into_iter().collect(),
                supported_profiles: SessionConfig::default().supported_profiles,
                required_profiles: SessionConfig::default().required_profiles,
                receiver_limits: ReceiverLimits::default(),
                resume: None,
            },
            0,
        )
        .unwrap();
    sessions
        .install_snapshot(&session.client, session.view_id, 1, 1)
        .unwrap();
    sessions
        .acknowledge_applied(&session.client, session.view_id, 1, true)
        .unwrap();
    sessions
        .acknowledge_presented(&session.client, session.view_id, 1)
        .unwrap();
    let lease = sessions
        .acquire_control(&session.client, session.view_id, [7; 16], 0)
        .unwrap();
    (sessions, session, lease)
}
fn family_bytes(
    family: CommandFamily,
    session: &rui_protocol_core::session::NegotiatedSession,
    lease: &rui_protocol_core::session::ControlLease,
    sequence: u64,
) -> Vec<u8> {
    let header = proto::CommandHeader {
        epoch: session.epoch.to_vec(),
        view_id: session.view_id.to_vec(),
        client_instance_id: session.client.client_instance_id.to_vec(),
        command_seq: sequence,
        control_lease_id: lease.lease_id.to_vec(),
        control_generation: lease.generation,
        observed_view_revision: 1,
    };
    match family {
        CommandFamily::Semantic => {
            let mut cmd = wire(sequence);
            cmd.epoch = header.epoch;
            cmd.view_id = header.view_id;
            cmd.control_lease_id = header.control_lease_id;
            cmd.control_generation = header.control_generation;
            cmd.encode_to_vec()
        }
        CommandFamily::RawInput => proto::RawInputBatch {
            header: Some(header),
            surface_id: 10,
            routing_token: vec![8; 16],
            presented_revision: 1,
            seat_id: "fixture-seat".into(),
            device_id: "fixture-pointer".into(),
            key_namespace: "linux.evdev".into(),
            events: vec![proto::RawEvent {
                timestamp_us: 100,
                event: Some(proto::raw_event::Event::Motion(proto::Point {
                    x: 2.0,
                    y: 3.0,
                })),
            }],
        }
        .encode_to_vec(),
        CommandFamily::TextComposition => proto::TextComposition {
            header: Some(header),
            target_id: 20,
            text_session_id: vec![6; 16],
            focus_generation: 1,
            base_text_version: 1,
            operation: Some(proto::text_composition::Operation::Begin(
                proto::ScalarRange { start: 1, end: 2 },
            )),
        }
        .encode_to_vec(),
        CommandFamily::Resize => proto::ResizeRequest {
            header: Some(header),
            surface_id: 10,
            surface_version: 1,
            width: 800.0,
            height: 600.0,
        }
        .encode_to_vec(),
    }
}
#[test]
fn common_cmd_one_contiguous_sequence_and_retry_space_for_all_families() {
    let (mut sessions, session, lease) = controlled();
    let mut ledger = CommandLedger::new(session.epoch, LedgerLimits::default()).unwrap();
    let calls = Cell::new(0);
    let families = [
        CommandFamily::Semantic,
        CommandFamily::RawInput,
        CommandFamily::TextComposition,
        CommandFamily::Resize,
    ];
    for (index, family) in families.into_iter().enumerate() {
        let sequence = index as u64 + 1;
        let bytes = family_bytes(family, &session, &lease, sequence);
        let result = ledger
            .submit_controlled(
                &mut sessions,
                &session.client,
                family,
                &bytes,
                0,
                |command| {
                    assert_eq!(command.family(), family);
                    assert_eq!(command.header().command_seq, sequence);
                    Ok(())
                },
                |_| {
                    calls.set(calls.get() + 1);
                    Ok(DispatchOutcome::Dispatched)
                },
            )
            .unwrap();
        assert_eq!(result.status, CommandStatus::Dispatched);
        assert_eq!(
            ledger
                .submit_controlled(
                    &mut sessions,
                    &session.client,
                    family,
                    &bytes,
                    1,
                    |_| panic!("retry validator"),
                    |_| panic!("retry dispatch")
                )
                .unwrap(),
            result
        );
    }
    assert_eq!(ledger.high_water(&session.client), 4);
    assert_eq!(calls.get(), 4);
    assert_eq!(ledger.producer_count(), 1);
    let raw = family_bytes(CommandFamily::RawInput, &session, &lease, 1);
    assert_eq!(
        ledger.submit_controlled(
            &mut sessions,
            &session.client,
            CommandFamily::RawInput,
            &raw,
            2,
            |_| Ok(()),
            |_| panic!()
        ),
        Err(LedgerError::ChangedBytes)
    );
    let gap = family_bytes(CommandFamily::Resize, &session, &lease, 6);
    assert_eq!(
        ledger.submit_controlled(
            &mut sessions,
            &session.client,
            CommandFamily::Resize,
            &gap,
            2,
            |_| Ok(()),
            |_| panic!()
        ),
        Err(LedgerError::SequenceGap)
    );
    let raw = family_bytes(CommandFamily::TextComposition, &session, &lease, 3);
    ledger.expire_outcomes(60_001);
    assert_eq!(
        ledger
            .submit_controlled(
                &mut sessions,
                &session.client,
                CommandFamily::TextComposition,
                &raw,
                0,
                |_| panic!(),
                |_| panic!()
            )
            .unwrap()
            .status,
        CommandStatus::OutcomeExpired
    );
}
#[test]
fn common_cmd_current_authority_and_lease_are_mandatory_and_rejects_consume() {
    let (mut sessions, session, lease) = controlled();
    let mut ledger = CommandLedger::new(session.epoch, LedgerLimits::default()).unwrap();
    let bytes = family_bytes(CommandFamily::RawInput, &session, &lease, 1);
    let result = ledger
        .submit_controlled(
            &mut sessions,
            &session.client,
            CommandFamily::RawInput,
            &bytes,
            0,
            |_| Err(Rejection::InvalidArguments),
            |_| panic!(),
        )
        .unwrap();
    assert_eq!(result.rejection, Some(Rejection::InvalidArguments));
    sessions.local_activity(1);
    let stale = family_bytes(CommandFamily::Resize, &session, &lease, 2);
    let result = ledger
        .submit_controlled(
            &mut sessions,
            &session.client,
            CommandFamily::Resize,
            &stale,
            2,
            |_| panic!("stale lease bypassed"),
            |_| panic!(),
        )
        .unwrap();
    assert_eq!(result.rejection, Some(Rejection::StaleControl));
    assert_eq!(ledger.high_water(&session.client), 2);
    // The trusted source unwinds its pending cleanup before another grant.
    assert!(sessions.take_cleanup().is_some());
    // A new lease cannot reinterpret the consumed stale request.
    let new = sessions
        .acquire_control(&session.client, session.view_id, [5; 16], 2)
        .unwrap();
    let changed = family_bytes(CommandFamily::Resize, &session, &new, 2);
    assert_eq!(
        ledger.submit_controlled(
            &mut sessions,
            &session.client,
            CommandFamily::Resize,
            &changed,
            2,
            |_| Ok(()),
            |_| panic!()
        ),
        Err(LedgerError::ChangedBytes)
    );
    let forged = AuthenticatedClient {
        principal: session.client.principal.clone(),
        client_instance_id: [99; 16],
    };
    assert_eq!(
        ledger.submit_controlled(
            &mut sessions,
            &forged,
            CommandFamily::Resize,
            &family_bytes(CommandFamily::Resize, &session, &new, 1),
            2,
            |_| Ok(()),
            |_| panic!()
        ),
        Err(LedgerError::WrongClient)
    );
}
#[test]
fn common_cmd_family_permissions_do_not_follow_from_semantic_control() {
    use rui_protocol_core::session::*;
    let (mut sessions, session, lease) = controlled();
    let mut authorization = VerifiedAuthorization {
        principal: "other".into(),
        policy_version: 1,
        permissions: Permissions {
            can_view: true,
            can_control: true,
            actions: ["Activate".into()].into_iter().collect(),
        },
    };
    let other = sessions
        .negotiate(
            Some(authorization.clone()),
            ClientHello {
                client_instance_id: [4; 16],
                supported_versions: [1].into_iter().collect(),
                supported_profiles: SessionConfig::default().supported_profiles,
                required_profiles: SessionConfig::default().required_profiles,
                receiver_limits: ReceiverLimits::default(),
                resume: None,
            },
            0,
        )
        .unwrap();
    sessions
        .release_control(
            &session.client,
            [10; 16],
            lease.lease_id,
            lease.generation,
            0,
        )
        .unwrap();
    assert!(sessions.take_cleanup().is_some());
    sessions
        .install_snapshot(&other.client, other.view_id, 1, 1)
        .unwrap();
    sessions
        .acknowledge_applied(&other.client, other.view_id, 1, true)
        .unwrap();
    sessions
        .acknowledge_presented(&other.client, other.view_id, 1)
        .unwrap();
    let lease = sessions
        .acquire_control(&other.client, other.view_id, [11; 16], 0)
        .unwrap();
    let mut ledger = CommandLedger::new(other.epoch, LedgerLimits::default()).unwrap();
    for (index, family) in [
        CommandFamily::RawInput,
        CommandFamily::TextComposition,
        CommandFamily::Resize,
    ]
    .into_iter()
    .enumerate()
    {
        let mut raw = family_bytes(family, &other, &lease, index as u64 + 1);
        // family_bytes obtains actual registered producer from the shared header.
        let result = ledger
            .submit_controlled(
                &mut sessions,
                &other.client,
                family,
                &raw,
                0,
                |_| panic!("missing family permission"),
                |_| panic!(),
            )
            .unwrap();
        assert_eq!(result.rejection, Some(Rejection::Unauthorized));
        raw.clear();
    }
    authorization.permissions.can_control = false; // Observer cannot borrow a controller's lease.
    let observer = sessions
        .negotiate(
            Some(authorization),
            ClientHello {
                client_instance_id: [5; 16],
                supported_versions: [1].into_iter().collect(),
                supported_profiles: SessionConfig::default().supported_profiles,
                required_profiles: SessionConfig::default().required_profiles,
                receiver_limits: ReceiverLimits::default(),
                resume: None,
            },
            0,
        )
        .unwrap();
    let result = ledger
        .submit_controlled(
            &mut sessions,
            &observer.client,
            CommandFamily::RawInput,
            &family_bytes(CommandFamily::RawInput, &observer, &lease, 1),
            0,
            |_| panic!(),
            |_| panic!(),
        )
        .unwrap();
    assert_eq!(result.rejection, Some(Rejection::Unauthorized));
}
#[test]
fn common_cmd_ambiguous_failure_or_panic_never_redispatches() {
    let (mut sessions, session, lease) = controlled();
    let mut ledger = CommandLedger::new(session.epoch, LedgerLimits::default()).unwrap();
    let count = Cell::new(0);
    let raw = family_bytes(CommandFamily::RawInput, &session, &lease, 1);
    let panic = catch_unwind(AssertUnwindSafe(|| {
        ledger.submit_controlled(
            &mut sessions,
            &session.client,
            CommandFamily::RawInput,
            &raw,
            0,
            |_| Ok(()),
            |_| {
                count.set(1);
                panic!("adapter after press")
            },
        )
    }));
    assert!(panic.is_err());
    assert_eq!(
        ledger
            .submit_controlled(
                &mut sessions,
                &session.client,
                CommandFamily::RawInput,
                &raw,
                0,
                |_| panic!(),
                |_| panic!()
            )
            .unwrap()
            .status,
        CommandStatus::OutcomeUnknown
    );
    let raw = family_bytes(CommandFamily::Resize, &session, &lease, 2);
    assert_eq!(
        ledger
            .submit_controlled(
                &mut sessions,
                &session.client,
                CommandFamily::Resize,
                &raw,
                0,
                |_| Ok(()),
                |_| Err(DispatchFailure::OutcomeUnknown)
            )
            .unwrap()
            .status,
        CommandStatus::OutcomeUnknown
    );
    assert_eq!(count.get(), 1);
}
#[test]
fn common_cmd_wire_preflight_rejects_required_variants_and_nested_expansion() {
    let (mut sessions, session, lease) = controlled();
    let mut ledger = CommandLedger::new(session.epoch, LedgerLimits::default()).unwrap();
    let mut unknown = family_bytes(CommandFamily::Semantic, &session, &lease, 1);
    unknown.extend_from_slice(&[138, 1, 0]); // unknown required argument 17
    assert_eq!(
        ledger.submit_controlled(
            &mut sessions,
            &session.client,
            CommandFamily::Semantic,
            &unknown,
            0,
            |_| Ok(()),
            |_| panic!()
        ),
        Err(LedgerError::Malformed)
    );
    assert_eq!(ledger.high_water(&session.client), 0);
    let mut optional = family_bytes(CommandFamily::Semantic, &session, &lease, 1);
    optional.extend_from_slice(&[186, 62, 0]); // optional 999
    assert_eq!(
        ledger
            .submit_controlled(
                &mut sessions,
                &session.client,
                CommandFamily::Semantic,
                &optional,
                0,
                |_| Ok(()),
                |_| Ok(DispatchOutcome::Dispatched)
            )
            .unwrap()
            .status,
        CommandStatus::Dispatched
    );
    let mut nested = [66, 0].repeat(257);
    assert_eq!(
        decode_family(CommandFamily::RawInput, &nested).unwrap_err(),
        LedgerError::Oversized
    );
    nested.clear();
    let mut merged = family_bytes(CommandFamily::Semantic, &session, &lease, 2);
    merged.extend([90, 0].repeat(40_000));
    assert_eq!(decode_command(&merged), Err(LedgerError::Oversized));
    let mut raw = proto::RawInputBatch::decode(
        family_bytes(CommandFamily::RawInput, &session, &lease, 2).as_slice(),
    )
    .unwrap();
    raw.events[0] = proto::RawEvent {
        timestamp_us: 1,
        event: None,
    };
    assert_eq!(
        decode_family(CommandFamily::RawInput, &raw.encode_to_vec()).unwrap_err(),
        LedgerError::Malformed
    );
    let mut event = raw.events[0].encode_to_vec();
    event.extend([82, 0]);
    assert!(rui_protocol_core::wire::decode_raw_input(
        &[66, 4, 8, 1, 82, 0],
        &rui_protocol_core::model::Limits::default()
    )
    .is_err());
    let mut edit = proto::TextComposition::decode(
        family_bytes(CommandFamily::TextComposition, &session, &lease, 2).as_slice(),
    )
    .unwrap();
    edit.operation = Some(proto::text_composition::Operation::CommitText(
        proto::CommitText {
            text: "PRIVATE-PREEDIT-SENTINEL".into(),
        },
    ));
    let decoded = decode_family(CommandFamily::TextComposition, &edit.encode_to_vec()).unwrap();
    assert!(!format!("{decoded:?}").contains("PRIVATE-PREEDIT-SENTINEL"));
}
