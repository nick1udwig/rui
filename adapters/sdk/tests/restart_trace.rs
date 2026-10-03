//! Exact public epoch continuity and private causal evidence from real SIGKILL.
use prost::Message;
use rui_producer_sdk::CaptureArtifact;
use rui_protocol_core::model::Record;
use rui_protocol_core::receiver::{ReceiveOutcome, ReceiverConfig, ViewReceiver};
use rui_protocol_core::resources::{ResourceContext, ResourceScope};
use rui_protocol_core::session::ReceiverLimits;
use rui_protocol_core::wire::proto as p;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::PathBuf};

#[test]
fn actual_broker_survives_source_kill_and_replays_public_retirement_and_fresh_effect() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gtk");
    let report: Value =
        serde_json::from_slice(&fs::read(base.join("restart-interaction.json")).unwrap()).unwrap();
    let evidence = &report["restart"];
    let root = base.join("restart-golden");
    let actual: BTreeSet<_> = fs::read_dir(root.join("restart"))
        .unwrap()
        .map(|e| format!("restart/{}", e.unwrap().file_name().to_string_lossy()))
        .collect();
    let hashes = report["retained_trace_sha256"].as_object().unwrap();
    assert_eq!(actual, hashes.keys().cloned().collect());
    for (name, hash) in hashes {
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(root.join(name)).unwrap())),
            hash.as_str().unwrap()
        );
        assert!(matches!(
            root.join(name).extension().unwrap().to_str().unwrap(),
            "bin" | "json" | "rvp"
        ));
    }
    assert_eq!(evidence["native_exit_signal"], libc::SIGKILL);
    assert_ne!(
        evidence["first_native_pid"],
        evidence["replacement_native_pid"]
    );
    assert_ne!(
        evidence["first_incarnation"],
        evidence["replacement_incarnation"]
    );
    assert_eq!(evidence["source_generation"], 2);
    assert_eq!(evidence["post_dispatch_receipt"]["native_dispatches"], 1);
    assert_eq!(evidence["failed_start_attempts"], 3);
    assert_eq!(evidence["host_budget_after_drop"], 0);

    let first_bytes = fs::read(root.join("restart/public-000001.bin")).unwrap();
    let first = p::Envelope::decode(first_bytes.as_slice()).unwrap();
    let context = ResourceContext {
        scope: ResourceScope {
            session_id: first.session_id.clone().try_into().unwrap(),
            epoch: first.epoch.clone().try_into().unwrap(),
            principal_id: [4; 16],
        },
        view_id: first.view_id.clone().try_into().unwrap(),
    };
    assert_eq!(
        evidence["broker_epoch"],
        first
            .epoch
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let mut receiver = ViewReceiver::new(
        context,
        ReceiverLimits::default(),
        ReceiverConfig::default(),
    )
    .unwrap();
    let session_id = evidence["stable_session_id"].as_u64().unwrap();
    let old: BTreeSet<u64> = evidence["old_public_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_u64().unwrap())
        .filter(|id| *id != session_id)
        .collect();
    let mut focus_generation = 0;
    for revision in 1..=5u64 {
        let bytes = fs::read(root.join(format!("restart/public-{revision:06}.bin"))).unwrap();
        let envelope = p::Envelope::decode(bytes.as_slice()).unwrap();
        assert_eq!(
            (envelope.session_id, envelope.epoch, envelope.view_id),
            (
                first.session_id.clone(),
                first.epoch.clone(),
                first.view_id.clone()
            )
        );
        assert!(
            matches!(receiver.receive_envelope((revision as u128).to_be_bytes(), &bytes).unwrap(),
            ReceiveOutcome::Applied { revision: applied } if applied == revision)
        );
        let state = receiver.state().unwrap();
        assert!(state.resource_refs().is_empty());
        assert!(state
            .records
            .values()
            .all(|r| !matches!(r, Record::Scene(_) | Record::Binding(_))));
        assert!(receiver.presented_revision().unwrap().is_none());
        let Record::Session(session) = &state.records[&session_id] else {
            panic!("broker Session survives")
        };
        assert!(session.focus_generation >= focus_generation);
        focus_generation = session.focus_generation;
        if revision >= 2 {
            assert!(old.is_subset(&state.retired));
        }
        if matches!(revision, 2 | 5) {
            assert_eq!(state.records.len(), 1);
            assert!(
                session.focus.is_none()
                    && session.focus_surface.is_none()
                    && session.cursor.is_none()
            );
        }
        let expected = match revision {
            2 => Some(&evidence["unavailable_state"]),
            4 => Some(&evidence["final_state"]),
            5 => Some(&evidence["idle_exit_state"]),
            _ => None,
        };
        if let Some(expected) = expected {
            assert_eq!(&serde_json::to_value(state).unwrap(), expected);
        }
    }
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);

    let mut causal_token = None;
    for (generation, revisions) in [(1, vec![1]), (2, vec![1, 2])] {
        let mut private_receiver = None;
        for revision in revisions {
            let artifact = CaptureArtifact::read(root.join(format!(
                "restart/snapshot-{generation:02}-{revision:06}.json"
            )))
            .unwrap();
            assert_eq!(artifact.manifest()["source_generation"], generation);
            let receiver = private_receiver.get_or_insert_with(|| artifact.receiver().unwrap());
            artifact.apply_trusted_fixture(receiver).unwrap();
            artifact.verify_native_glyph_evidence(receiver).unwrap();
            artifact.verify_vector_audit(receiver).unwrap();
            if generation == 2 && revision == 2 {
                assert_eq!(artifact.manifest()["command_cause"]["source_generation"], 2);
                assert_eq!(
                    artifact.manifest()["command_cause"]["authority_gate"],
                    "core-ledger-admitted"
                );
                causal_token = Some(
                    artifact.manifest()["capture_token"]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                );
            }
        }
        let receiver = private_receiver.as_mut().unwrap();
        receiver.close();
        assert_eq!(receiver.memory_used(), 0);
    }
    let commands = evidence["commands"].as_array().unwrap();
    let original = commands[0]["exact_semantic_wire_hex"].as_str().unwrap();
    for index in [1, 2, 7] {
        assert_eq!(commands[index]["status"], "OutcomeUnknown");
        assert_eq!(commands[index]["exact_semantic_wire_hex"], original);
        assert_eq!(commands[index]["retained_result_equal"], true);
    }
    assert_eq!(commands[3]["rejection"], "MissingTarget");
    assert_eq!(commands[4]["status"], "Failed");
    assert_eq!(commands[4]["native_dispatches"], 0);
    assert_eq!(commands[5]["retained_result_equal"], true);
    assert_eq!(commands[6]["status"], "Applied");
    assert_eq!(commands[6]["published_revision"], 4);
    assert_eq!(
        commands[6]["source_commit"].as_str(),
        causal_token.as_deref()
    );
    assert_eq!(commands[6]["native_dispatches"], 1);
}
