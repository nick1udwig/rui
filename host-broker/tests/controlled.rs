use prost::Message;
use rui_host_broker::*;
use rui_protocol_core::commands::{CommandStatus, LedgerError, Rejection};
use rui_protocol_core::model::{Primitive, Record, Sensitivity, TextStatus, TypedValue};
use rui_protocol_core::receiver::{ReceiveOutcome, ReceiverConfig, ViewReceiver};
use rui_protocol_core::resources::{ResourceContext, ResourceScope};
use rui_protocol_core::session::*;
use rui_protocol_core::wire::proto as p;
use std::collections::BTreeSet;

const SENTINEL: &str = "UNIQUE-SECRET-SENTINEL";
fn auth(name: &str, controller: bool) -> VerifiedAuthorization {
    VerifiedAuthorization {
        principal: name.into(),
        policy_version: 1,
        permissions: Permissions {
            can_view: true,
            can_control: controller,
            actions: if controller {
                ["Activate", "ReplaceText", READ_SECRET_PERMISSION]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            } else {
                BTreeSet::new()
            },
        },
    }
}
fn hello(id: u8) -> ClientHello {
    ClientHello {
        client_instance_id: [id; 16],
        supported_versions: [1].into_iter().collect(),
        supported_profiles: SessionConfig::default().supported_profiles,
        required_profiles: SessionConfig::default().required_profiles,
        receiver_limits: ReceiverLimits::default(),
        resume: None,
    }
}
fn context(session: &NegotiatedSession) -> ResourceContext {
    ResourceContext {
        scope: ResourceScope {
            session_id: session.session_id,
            epoch: session.epoch,
            principal_id: session.client.client_instance_id,
        },
        view_id: session.view_id,
    }
}
fn receiver(session: &NegotiatedSession) -> ViewReceiver {
    ViewReceiver::new(
        context(session),
        session.limits.clone(),
        ReceiverConfig::default(),
    )
    .unwrap()
}
fn open(host: &mut ControlledHost, id: u8, controller: bool) -> (NegotiatedSession, ViewReceiver) {
    let session = host
        .negotiate(
            auth(
                if controller { "controller" } else { "observer" },
                controller,
            ),
            hello(id),
            0,
        )
        .unwrap();
    let mut receiver = receiver(&session);
    let snapshot = host.checkpoint(&session.client, session.view_id).unwrap();
    assert!(matches!(
        receiver.receive_envelope([1; 16], &snapshot).unwrap(),
        ReceiveOutcome::Applied { revision: 1 }
    ));
    ack(host, &session, &mut receiver);
    (session, receiver)
}
fn ack(host: &mut ControlledHost, session: &NegotiatedSession, receiver: &mut ViewReceiver) {
    let revision = receiver.state().unwrap().revision;
    host.acknowledge_applied(&session.client, session.view_id, revision)
        .unwrap();
    let presented = receiver.mark_presented(revision).unwrap();
    host.acknowledge_presented(&session.client, session.view_id, presented.revision)
        .unwrap();
}
fn host(secret: &str) -> ControlledHost {
    ControlledHost::new(
        HostConfig {
            max_text_bytes: 256,
            ..HostConfig::default()
        },
        secret.into(),
    )
    .unwrap()
}
fn activate(
    session: &NegotiatedSession,
    lease: &ControlLease,
    seq: u64,
    version: u64,
    observed: u64,
) -> p::SemanticCommand {
    p::SemanticCommand {
        epoch: session.epoch.to_vec(),
        view_id: session.view_id.to_vec(),
        client_instance_id: session.client.client_instance_id.to_vec(),
        command_seq: seq,
        control_lease_id: lease.lease_id.to_vec(),
        control_generation: lease.generation,
        target_id: COUNTER_ID,
        action_id: "Activate".into(),
        action_schema_version: 1,
        observed_view_revision: observed,
        preconditions: Some(p::Preconditions {
            target_version: Some(version),
            ..Default::default()
        }),
        arguments: Some(p::semantic_command::Arguments::NoArguments(
            p::EmptyArguments {},
        )),
    }
}
fn replace(
    session: &NegotiatedSession,
    lease: &ControlLease,
    seq: u64,
    version: u64,
    start: u64,
    end: u64,
    text: &str,
) -> p::SemanticCommand {
    let mut command = activate(session, lease, seq, version, 1);
    command.target_id = SECRET_ID;
    command.action_id = "ReplaceText".into();
    command.preconditions = Some(p::Preconditions {
        target_version: Some(version),
        text_version: Some(version),
        focus_generation: Some(1),
        model_version: None,
    });
    command.arguments = Some(p::semantic_command::Arguments::ReplaceText(
        p::TextArguments {
            start,
            end,
            text: text.into(),
        },
    ));
    command
}
fn receive_delta(
    host: &mut ControlledHost,
    session: &NegotiatedSession,
    receiver: &mut ViewReceiver,
    revision: u64,
) -> Vec<u8> {
    let bytes = host
        .take_outbound(&session.client, session.view_id)
        .unwrap()
        .unwrap();
    assert!(
        matches!(receiver.receive_envelope([revision as u8;16],&bytes).unwrap(),ReceiveOutcome::Applied {revision:r} if r==revision)
    );
    bytes.to_vec()
}
fn counter(receiver: &ViewReceiver) -> i64 {
    match &receiver.state().unwrap().records[&COUNTER_ID] {
        Record::Semantic(node) => match node.value {
            Some(TypedValue::Integer(value)) => value,
            _ => panic!(),
        },
        _ => panic!(),
    }
}
fn assert_private(receiver: &ViewReceiver) {
    match &receiver.state().unwrap().records[&SECRET_ID] {
        Record::Semantic(node) => {
            assert_eq!(node.sensitivity, Sensitivity::Redacted);
            assert!(node.value.is_none());
            assert!(node.actions.is_empty());
        }
        _ => panic!(),
    }
    match &receiver.state().unwrap().records[&SECRET_TEXT_ID] {
        Record::Text(text) => {
            assert_eq!(text.status, TextStatus::Redacted);
            assert!(text.content.is_none());
            assert!(text.selection.is_none());
            assert!(!text.editable);
        }
        _ => panic!(),
    }
    assert!(receiver.state().unwrap().records.values().all(|record|!matches!(record,Record::Scene(scene) if matches!(scene.primitive,Primitive::GlyphRun {..}))));
    assert!(!format!("{:?}", receiver.state().unwrap()).contains(SENTINEL));
}
fn assert_no_secret(bytes: &[u8]) {
    assert!(!bytes
        .windows(SENTINEL.len())
        .any(|window| window == SENTINEL.as_bytes()));
}

#[test]
fn controller_and_observer_converge_once_on_retry_with_real_source_commit() {
    let mut host = host(SENTINEL);
    let (a, mut ar) = open(&mut host, 3, true);
    let (b, mut br) = open(&mut host, 4, false);
    assert_private(&br);
    assert_no_secret(&host.snapshot(&b.client, b.view_id).unwrap());
    let lease = host
        .acquire_control(&a.client, a.view_id, [1; 16], 0)
        .unwrap();
    let raw = activate(&a, &lease, 1, 1, 1).encode_to_vec();
    let result = host.submit(&a.client, &raw, 1).unwrap();
    assert_eq!(result.status, CommandStatus::Applied);
    let evidence = result.evidence.as_ref().unwrap();
    assert_eq!(evidence.source_commit(), "fixture-commit-2");
    assert_eq!(evidence.resulting_view_revision(), Some(2));
    let before = host.diagnostics();
    assert_eq!(host.submit(&a.client, &raw, 2).unwrap(), result);
    assert_eq!(host.diagnostics(), before);
    let delta_a = receive_delta(&mut host, &a, &mut ar, 2);
    let delta_b = receive_delta(&mut host, &b, &mut br, 2);
    assert_no_secret(&delta_b);
    assert_eq!(counter(&ar), 1);
    assert_eq!(counter(&br), 1);
    assert_private(&br);
    for bytes in [&delta_a, &delta_b] {
        let envelope = p::Envelope::decode(bytes.as_slice()).unwrap();
        assert_eq!(envelope.kind, p::MessageKind::Transaction as i32);
        let transaction = p::Transaction::decode(envelope.payload.as_slice()).unwrap();
        assert!(transaction.mutations.iter().any(|m|matches!(&m.mutation,Some(p::mutation::Mutation::Put(p::Record {record:Some(p::record::Record::Semantic(v))})) if v.id==COUNTER_ID && v.version==2)));
        assert!(transaction.mutations.iter().any(|m|matches!(&m.mutation,Some(p::mutation::Mutation::Put(p::Record {record:Some(p::record::Record::Scene(v))})) if v.id==COUNTER_SCENE_ID && v.version==2)));
        assert!(transaction.mutations.iter().any(|m|matches!(&m.mutation,Some(p::mutation::Mutation::Put(p::Record {record:Some(p::record::Record::Surface(v))})) if v.source_capture_tokens==vec![2u64.to_be_bytes().to_vec()])));
    }
    assert!(host.take_outbound(&a.client, a.view_id).unwrap().is_none());
    assert!(host.take_outbound(&b.client, b.view_id).unwrap().is_none());
    let mut changed = activate(&a, &lease, 1, 1, 1);
    changed.target_id = SECRET_ID;
    assert_eq!(
        host.submit(&a.client, &changed.encode_to_vec(), 3),
        Err(HostError::Ledger(LedgerError::ChangedBytes))
    );
}
#[test]
fn scalar_edit_noop_and_privacy_are_truthful_across_snapshots_deltas_and_resume() {
    let initial = format!("A😀e\u{301}אב-{SENTINEL}");
    let mut host = host(&initial);
    let (a, mut ar) = open(&mut host, 3, true);
    let (b, mut br) = open(&mut host, 4, false);
    let lease = host
        .acquire_control(&a.client, a.view_id, [1; 16], 0)
        .unwrap();
    let edited = format!("A🦀e\u{301}אב-{SENTINEL}");
    for seq in 1..=3 {
        let command = if seq == 1 {
            replace(&a, &lease, seq, 1, 1, 2, "🦀")
        } else {
            replace(
                &a,
                &lease,
                seq,
                2,
                0,
                edited.chars().count() as u64,
                &edited,
            )
        };
        let result = host
            .submit(&a.client, &command.encode_to_vec(), seq)
            .unwrap();
        assert_eq!(result.status, CommandStatus::Applied);
        assert!(!format!("{result:?}").contains(SENTINEL));
        receive_delta(&mut host, &a, &mut ar, seq + 1);
        match &ar.state().unwrap().records[&SECRET_TEXT_ID] {
            Record::Text(text) => {
                assert_eq!(text.content.as_deref(), Some(edited.as_str()));
                assert_eq!(text.text_version, 2);
            }
            _ => panic!(),
        }
    }
    // Observer retained revision 1; all three independently redacted envelopes
    // remain available for exact-byte recovery without text or glyph echo.
    host.disconnect(&b.client, 4).unwrap();
    br.disconnect();
    let mut reconnect = hello(4);
    reconnect.resume = Some(ResumeRequest {
        epoch: b.epoch,
        view_id: b.view_id,
        acknowledged_revision: 1,
    });
    let resumed = host
        .negotiate(auth("observer", false), reconnect, 5)
        .unwrap();
    br.reconnect(context(&resumed)).unwrap();
    match resumed.resume {
        ResumeDisposition::Accepted {
            transactions,
            through_revision,
            ..
        } => {
            assert_eq!(through_revision, 4);
            assert_eq!(transactions.len(), 3);
            for (i, bytes) in transactions.iter().enumerate() {
                assert_no_secret(bytes);
                br.receive_envelope([10 + i as u8; 16], bytes).unwrap();
                assert_private(&br);
            }
        }
        _ => panic!("history must resume"),
    }
    let read = host.snapshot(&b.client, b.view_id).unwrap();
    assert_no_secret(&read);
    let mut fresh = receiver(&b);
    fresh.receive_envelope([20; 16], &read).unwrap();
    assert_eq!(fresh.state().unwrap(), br.state().unwrap());
    assert!(!format!("{:?}", host.diagnostics()).contains(SENTINEL));
    assert_eq!(host.diagnostics().source_commit, 4);
    assert_eq!(
        host.get_command_result(&a.client, a.view_id, 2, 6)
            .unwrap()
            .evidence
            .unwrap()
            .source_commit(),
        "fixture-commit-3"
    );
}
#[test]
fn observers_disabled_targets_stale_text_and_old_leases_cannot_execute() {
    let mut host = host(SENTINEL);
    let (a, mut ar) = open(&mut host, 3, true);
    let (b, mut br) = open(&mut host, 4, false);
    let lease = host
        .acquire_control(&a.client, a.view_id, [1; 16], 0)
        .unwrap();
    let observer = activate(&b, &lease, 1, 1, 1);
    let result = host
        .submit(&b.client, &observer.encode_to_vec(), 1)
        .unwrap();
    assert_eq!(result.status, CommandStatus::Rejected);
    assert_eq!(host.diagnostics().counter, 0);
    host.set_counter_enabled(false, 2).unwrap();
    receive_delta(&mut host, &a, &mut ar, 2);
    receive_delta(&mut host, &b, &mut br, 2);
    let result = host
        .submit(&a.client, &activate(&a, &lease, 1, 1, 1).encode_to_vec(), 2)
        .unwrap();
    assert_eq!(result.rejection, Some(Rejection::DisabledTarget));
    let mut stale = replace(&a, &lease, 2, 1, 0, 0, "bad");
    stale.preconditions.as_mut().unwrap().text_version = Some(0);
    let result = host.submit(&a.client, &stale.encode_to_vec(), 2).unwrap();
    assert_eq!(result.rejection, Some(Rejection::StaleText));
    host.set_counter_enabled(true, 3).unwrap();
    host.local_activity(3);
    host.take_cleanup().unwrap();
    let result = host
        .submit(&a.client, &activate(&a, &lease, 3, 3, 1).encode_to_vec(), 3)
        .unwrap();
    assert_eq!(result.rejection, Some(Rejection::StaleControl));
    let new = host
        .acquire_control(&a.client, a.view_id, [2; 16], 3)
        .unwrap();
    assert!(new.generation > lease.generation);
    let result = host
        .submit(&a.client, &activate(&a, &new, 4, 3, 1).encode_to_vec(), 4)
        .unwrap();
    assert_eq!(result.status, CommandStatus::Applied);
    assert_eq!(host.diagnostics().counter, 1);
}
#[test]
fn disconnected_source_changes_require_explicit_checkpoint_and_preserve_stable_ids() {
    let mut host = host(SENTINEL);
    let (a, mut ar) = open(&mut host, 3, true);
    let old = host
        .acquire_control(&a.client, a.view_id, [1; 16], 0)
        .unwrap();
    host.disconnect(&a.client, 1).unwrap();
    ar.disconnect();
    host.take_cleanup().unwrap();
    let b = host
        .negotiate(auth("other-controller", true), hello(4), 2)
        .unwrap();
    let mut br = receiver(&b);
    let checkpoint = host.checkpoint(&b.client, b.view_id).unwrap();
    br.receive_envelope([1; 16], &checkpoint).unwrap();
    ack(&mut host, &b, &mut br);
    let lease = host
        .acquire_control(&b.client, b.view_id, [2; 16], 2)
        .unwrap();
    for seq in 1..=2 {
        host.submit(
            &b.client,
            &replace(&b, &lease, seq, seq, 0, 0, "😀").encode_to_vec(),
            seq + 2,
        )
        .unwrap();
        receive_delta(&mut host, &b, &mut br, seq + 1);
    }
    let mut reconnect = hello(3);
    reconnect.resume = Some(ResumeRequest {
        epoch: a.epoch,
        view_id: a.view_id,
        acknowledged_revision: 1,
    });
    let resumed = host
        .negotiate(auth("controller", true), reconnect, 5)
        .unwrap();
    assert!(matches!(
        resumed.resume,
        ResumeDisposition::ResetRequired(_)
    ));
    assert_eq!(
        host.acquire_control(&a.client, a.view_id, [3; 16], 5),
        Err(HostError::CheckpointRequired)
    );
    assert_eq!(
        host.snapshot(&a.client, a.view_id),
        Err(HostError::CheckpointRequired)
    );
    let snapshot = host.checkpoint(&a.client, a.view_id).unwrap();
    ar.reconnect(context(&resumed)).unwrap();
    ar.receive_envelope([20; 16], &snapshot).unwrap();
    ack(&mut host, &resumed, &mut ar);
    match &ar.state().unwrap().records[&SECRET_TEXT_ID] {
        Record::Text(text) => {
            assert_eq!(text.text_version, 3);
            assert_eq!(
                text.content.as_deref(),
                Some(format!("😀😀{SENTINEL}").as_str())
            );
        }
        _ => panic!(),
    }
    assert_eq!(
        host.acquire_control(&a.client, a.view_id, [1; 16], 5),
        Err(HostError::Session(SessionError::StaleRequest))
    );
    let old_command = activate(&a, &old, 1, 1, 1);
    assert_eq!(
        host.submit(&a.client, &old_command.encode_to_vec(), 5)
            .unwrap()
            .rejection,
        Some(Rejection::StaleControl)
    );
}
#[test]
fn permission_change_replaces_projection_and_does_not_resurrect_old_secret_history() {
    let mut host = host(SENTINEL);
    let (a, mut ar) = open(&mut host, 3, true);
    let lease = host
        .acquire_control(&a.client, a.view_id, [1; 16], 0)
        .unwrap();
    host.submit(
        &a.client,
        &replace(&a, &lease, 1, 1, 0, 0, "new-").encode_to_vec(),
        1,
    )
    .unwrap();
    receive_delta(&mut host, &a, &mut ar, 2);
    let mut reduced = auth("controller", false);
    reduced.policy_version = 2;
    let mut request = hello(3);
    request.resume = Some(ResumeRequest {
        epoch: a.epoch,
        view_id: a.view_id,
        acknowledged_revision: 1,
    });
    let observer = host.negotiate(reduced, request, 2).unwrap();
    assert_ne!(observer.view_id, a.view_id);
    assert!(matches!(
        observer.resume,
        ResumeDisposition::ResetRequired(ResetReason::ViewChanged)
    ));
    let bytes = host.checkpoint(&observer.client, observer.view_id).unwrap();
    assert_no_secret(&bytes);
    let mut receiver = receiver(&observer);
    receiver.receive_envelope([3; 16], &bytes).unwrap();
    assert_private(&receiver);
    assert!(matches!(
        host.snapshot(&a.client, a.view_id),
        Err(HostError::Session(SessionError::WrongView))
    ));
}
#[test]
fn publication_failure_after_app_commit_has_no_invented_view_revision() {
    let mut host = host(SENTINEL);
    let mut constrained = hello(3);
    constrained.receiver_limits.transaction_bytes = 1;
    let a = host
        .negotiate(auth("controller", true), constrained, 0)
        .unwrap();
    let mut ar = receiver(&a);
    let snapshot = host.checkpoint(&a.client, a.view_id).unwrap();
    ar.receive_envelope([1; 16], &snapshot).unwrap();
    ack(&mut host, &a, &mut ar);
    let (b, mut br) = open(&mut host, 4, false);
    let lease = host
        .acquire_control(&a.client, a.view_id, [1; 16], 0)
        .unwrap();
    let result = host
        .submit(&a.client, &activate(&a, &lease, 1, 1, 1).encode_to_vec(), 1)
        .unwrap();
    assert_eq!(result.status, CommandStatus::Applied);
    assert_eq!(result.evidence.unwrap().resulting_view_revision(), None);
    assert_eq!(host.diagnostics().counter, 1);
    assert_eq!(
        host.view_status(&a.client, a.view_id).unwrap().closed,
        Some(PublicationFailure::ReceiverLimit)
    );
    receive_delta(&mut host, &b, &mut br, 2);
    assert_eq!(counter(&br), 1);
}
#[test]
fn slow_observer_closes_independently_and_metadata_remains_bounded() {
    let config = HostConfig {
        session: SessionConfig {
            max_clients: 2,
            max_history_records: 3,
            ..SessionConfig::default()
        },
        max_text_bytes: 64,
        ..HostConfig::default()
    };
    let mut host = ControlledHost::new(config, SENTINEL.into()).unwrap();
    let (a, mut ar) = open(&mut host, 3, true);
    let mut slow = hello(4);
    slow.receiver_limits.outbound_queue_bytes = 1000;
    let b = host.negotiate(auth("observer", false), slow, 0).unwrap();
    let snapshot = host.checkpoint(&b.client, b.view_id).unwrap();
    drop(snapshot);
    host.acknowledge_applied(&b.client, b.view_id, 1).unwrap();
    host.acknowledge_presented(&b.client, b.view_id, 1).unwrap();
    let lease = host
        .acquire_control(&a.client, a.view_id, [1; 16], 0)
        .unwrap();
    for seq in 1..=20 {
        let result = host
            .submit(
                &a.client,
                &activate(&a, &lease, seq, seq, 1).encode_to_vec(),
                seq,
            )
            .unwrap();
        assert_eq!(result.status, CommandStatus::Applied);
        receive_delta(&mut host, &a, &mut ar, seq + 1);
    }
    assert_eq!(counter(&ar), 20);
    assert_eq!(
        host.view_status(&b.client, b.view_id).unwrap().closed,
        Some(PublicationFailure::SlowConsumer)
    );
    assert!(matches!(
        host.take_outbound(&b.client, b.view_id),
        Err(HostError::ViewClosed(PublicationFailure::SlowConsumer))
    ));
    assert_eq!(host.diagnostics().clients, 2);
    assert_eq!(host.diagnostics().closed_views, 1);
    assert!(matches!(
        host.negotiate(auth("third", false), hello(5), 30),
        Err(HostError::Session(SessionError::RetentionExhausted))
    ));
}
#[test]
fn snapshot_reads_preserve_history_and_the_last_handle_stays_memory_charged() {
    let mut host = ControlledHost::new(
        HostConfig {
            max_text_bytes: 64,
            ..HostConfig::default()
        },
        SENTINEL.into(),
    )
    .unwrap();
    let (a, mut ar) = open(&mut host, 3, true);
    let lease = host
        .acquire_control(&a.client, a.view_id, [1; 16], 0)
        .unwrap();
    host.submit(&a.client, &activate(&a, &lease, 1, 1, 1).encode_to_vec(), 1)
        .unwrap();
    receive_delta(&mut host, &a, &mut ar, 2);
    let before = host.retained_memory_bytes(&a.client, a.view_id).unwrap();
    let read = host.snapshot(&a.client, a.view_id).unwrap();
    let clone = read.clone();
    assert!(host.retained_memory_bytes(&a.client, a.view_id).unwrap() > before);
    drop(read);
    assert!(host.retained_memory_bytes(&a.client, a.view_id).unwrap() > before);
    drop(clone);
    assert_eq!(
        host.retained_memory_bytes(&a.client, a.view_id).unwrap(),
        before
    );
    let mut request = hello(3);
    request.resume = Some(ResumeRequest {
        epoch: a.epoch,
        view_id: a.view_id,
        acknowledged_revision: 1,
    });
    host.disconnect(&a.client, 2).unwrap();
    let resumed = host
        .negotiate(auth("controller", true), request, 3)
        .unwrap();
    assert!(matches!(
        resumed.resume,
        ResumeDisposition::Accepted {
            through_revision: 2,
            ..
        }
    ));
}
#[test]
fn lowered_envelope_ceiling_denies_oversized_retained_resume_envelopes() {
    let mut host = host(SENTINEL);
    let (a, mut ar) = open(&mut host, 3, true);
    let (b, _) = open(&mut host, 4, false);
    let lease = host
        .acquire_control(&a.client, a.view_id, [1; 16], 0)
        .unwrap();
    host.submit(&a.client, &activate(&a, &lease, 1, 1, 1).encode_to_vec(), 1)
        .unwrap();
    receive_delta(&mut host, &a, &mut ar, 2);
    host.disconnect(&b.client, 2).unwrap();
    let mut reconnect = hello(4);
    reconnect.receiver_limits.envelope_bytes = 128;
    reconnect.resume = Some(ResumeRequest {
        epoch: b.epoch,
        view_id: b.view_id,
        acknowledged_revision: 1,
    });
    let resumed = host
        .negotiate(auth("observer", false), reconnect, 3)
        .unwrap();
    assert!(matches!(
        resumed.resume,
        ResumeDisposition::ResetRequired(_)
    ));
    assert!(
        host.view_status(&b.client, b.view_id)
            .unwrap()
            .checkpoint_required
    );
}
#[test]
fn source_text_and_snapshot_outputs_have_hard_bounds_without_secret_errors() {
    let config = HostConfig {
        max_text_bytes: 32,
        ..HostConfig::default()
    };
    let mut host = ControlledHost::new(config, SENTINEL.into()).unwrap();
    let (a, _) = open(&mut host, 3, true);
    let lease = host
        .acquire_control(&a.client, a.view_id, [1; 16], 0)
        .unwrap();
    let oversized = replace(&a, &lease, 1, 1, 0, 0, &"x".repeat(33));
    let result = host
        .submit(&a.client, &oversized.encode_to_vec(), 1)
        .unwrap();
    assert_eq!(result.rejection, Some(Rejection::InvalidArguments));
    assert_eq!(host.diagnostics().source_commit, 1);
    let mut held = Vec::new();
    let limit = a.limits.view_memory_bytes;
    // Negotiate a small but sufficient view budget, then keep read handles alive
    // until the same core budget refuses another snapshot.
    host.disconnect(&a.client, 2).unwrap();
    host.take_cleanup().unwrap();
    let mut smaller = hello(3);
    // The shared ceiling includes a conservative temporary capture workspace
    // independently of live projection state and retained snapshot handles.
    smaller.receiver_limits.view_memory_bytes = 80 * 1024;
    smaller.resume = Some(ResumeRequest {
        epoch: a.epoch,
        view_id: a.view_id,
        acknowledged_revision: 1,
    });
    let reduced = host
        .negotiate(auth("controller", true), smaller, 3)
        .unwrap();
    loop {
        match host.snapshot(&reduced.client, reduced.view_id) {
            Ok(snapshot) => held.push(snapshot),
            Err(HostError::Session(SessionError::Oversized)) => break,
            Err(error) => panic!("{error}"),
        }
    }
    assert!(!held.is_empty());
    assert!(held.len() < 32);
    assert!(
        host.retained_memory_bytes(&reduced.client, reduced.view_id)
            .unwrap()
            <= reduced.limits.view_memory_bytes
    );
    assert!(reduced.limits.view_memory_bytes < limit);
    assert!(!format!("{:?}", host.diagnostics()).contains(SENTINEL));
}

#[test]
fn current_text_still_requires_current_focus_and_control_does_not_grant_secret_read() {
    let mut host = host(SENTINEL);
    let (a, _) = open(&mut host, 3, true);
    let lease = host
        .acquire_control(&a.client, a.view_id, [1; 16], 0)
        .unwrap();
    let mut stale_focus = replace(&a, &lease, 1, 1, 0, 0, "bad");
    stale_focus.preconditions.as_mut().unwrap().focus_generation = Some(2);
    let result = host
        .submit(&a.client, &stale_focus.encode_to_vec(), 1)
        .unwrap();
    assert_eq!(result.rejection, Some(Rejection::StaleFocus));
    let mut missing_focus = replace(&a, &lease, 2, 1, 0, 0, "bad");
    missing_focus
        .preconditions
        .as_mut()
        .unwrap()
        .focus_generation = None;
    let result = host
        .submit(&a.client, &missing_focus.encode_to_vec(), 1)
        .unwrap();
    assert_eq!(result.rejection, Some(Rejection::StaleFocus));
    assert_eq!(host.diagnostics().source_commit, 1);
    host.disconnect(&a.client, 2).unwrap();
    host.take_cleanup().unwrap();
    let mut limited = auth("limited-controller", true);
    limited.permissions.actions.remove(READ_SECRET_PERMISSION);
    let b = host.negotiate(limited, hello(4), 3).unwrap();
    let mut br = receiver(&b);
    let snapshot = host.checkpoint(&b.client, b.view_id).unwrap();
    assert_no_secret(&snapshot);
    br.receive_envelope([3; 16], &snapshot).unwrap();
    assert_private(&br);
    ack(&mut host, &b, &mut br);
    let lease = host
        .acquire_control(&b.client, b.view_id, [2; 16], 3)
        .unwrap();
    let result = host
        .submit(
            &b.client,
            &replace(&b, &lease, 1, 1, 0, 0, "bad").encode_to_vec(),
            4,
        )
        .unwrap();
    assert_eq!(result.rejection, Some(Rejection::UnsupportedAction));
    let result = host
        .submit(&b.client, &activate(&b, &lease, 2, 1, 1).encode_to_vec(), 4)
        .unwrap();
    assert_eq!(result.status, CommandStatus::Applied);
    let delta = receive_delta(&mut host, &b, &mut br, 2);
    assert_no_secret(&delta);
    assert_private(&br);
    assert_eq!(counter(&br), 1);
}
