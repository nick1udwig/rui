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
    optional.extend_from_slice(&[0xa0, 0x06, 0x01]);
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
