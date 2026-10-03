//! Actual delayed native dispatch observes the independently invalidated fence.
use rui_producer_sdk::CaptureArtifact;
use rui_protocol_core::model::Record;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::PathBuf};

#[test]
fn retired_native_fence_denies_delayed_action_and_exact_retry() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gtk");
    let report: Value =
        serde_json::from_slice(&fs::read(base.join("native-fence-interaction.json")).unwrap())
            .unwrap();
    let root = base.join("native-fence-golden");
    let directory = root.join("native-fence");
    let actual: BTreeSet<_> = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| {
            format!(
                "native-fence/{}",
                entry.unwrap().file_name().to_string_lossy()
            )
        })
        .collect();
    let hashes = report["retained_trace_sha256"].as_object().unwrap();
    assert_eq!(actual, hashes.keys().cloned().collect());
    assert_eq!(actual.len(), 42);
    for (name, hash) in hashes {
        let path = root.join(name);
        assert!(matches!(
            path.extension().unwrap().to_str().unwrap(),
            "bin" | "json" | "rvp"
        ));
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(path).unwrap())),
            hash.as_str().unwrap()
        );
    }
    let evidence = &report["native_fence"];
    let commands = evidence["commands"].as_array().unwrap();
    assert_eq!(commands.len(), 3);
    for command in &commands[..2] {
        assert_eq!(command["status"], "Failed");
        assert_eq!(command["native_dispatches"], 0);
    }
    assert!(commands[0]["source_commit"].is_null());
    assert!(commands[0]["published_revision"].is_null());
    assert_eq!(
        commands[1]["exact_semantic_wire_hex"],
        commands[0]["exact_semantic_wire_hex"]
    );
    assert_eq!(commands[1]["retained_result_equal"], true);
    assert_eq!(commands[1]["received_native_counters_unchanged"], true);
    assert_eq!(commands[2]["status"], "Applied");
    assert_eq!(commands[2]["native_dispatches"], 1);
    assert_eq!(evidence["native_dispatches"], 1);
    assert_eq!(evidence["native_callbacks"], 4);
    assert_eq!(evidence["retired_generation"], 1);
    assert_eq!(evidence["fresh_generation"], 2);
    let receipt = &evidence["native_fence_receipt"];
    assert_eq!(receipt["reason"], "retired-native-lifecycle-fence");
    assert_eq!(receipt["fence_generation"], 0);
    assert_eq!(
        receipt["control_generation"],
        evidence["retired_generation"]
    );
    assert_eq!(receipt["source_generation"], 1);
    assert_eq!(receipt["private_request_id"], 4);
    assert_eq!(receipt["native_dispatches"], 0);
    let queued = &evidence["native_fence_queued"];
    assert_eq!(queued["private_request_id"], receipt["private_request_id"]);
    assert_eq!(queued["source_generation"], receipt["source_generation"]);
    assert_eq!(queued["control_generation"], evidence["retired_generation"]);
    assert_eq!(queued["fence_generation"], evidence["retired_generation"]);
    assert_eq!(queued["native_dispatches"], 0);
    assert!(
        queued["observed_monotonic_ns"].as_u64().unwrap()
            < receipt["observed_monotonic_ns"].as_u64().unwrap()
    );

    let initial = CaptureArtifact::read(directory.join("snapshot-01-000001.json")).unwrap();
    let mut receiver = initial.receiver().unwrap();
    let mut previous_content = None;
    let mut modal_id = None;
    for revision in 1..=2 {
        let artifact =
            CaptureArtifact::read(directory.join(format!("snapshot-01-{revision:06}.json")))
                .unwrap();
        artifact.apply_trusted_fixture(&mut receiver).unwrap();
        artifact.verify_vector_audit(&receiver).unwrap();
        let state = receiver.state().unwrap();
        if revision == 1 {
            modal_id = state.records.values().find_map(|record| match record {
                Record::Surface(surface) if surface.modal_for.is_some() => Some(surface.id),
                _ => None,
            });
        }
        let Record::Surface(modal) = &state.records[&modal_id.unwrap()] else {
            panic!("modal surface")
        };
        assert_eq!(modal.mapped, revision == 1);
        let content: Vec<_> = state
            .records
            .values()
            .filter_map(|record| match record {
                Record::Text(text) => Some((text.id, text.content.clone(), text.text_version)),
                _ => None,
            })
            .collect();
        if revision == 1 {
            previous_content = Some(content);
        } else {
            assert_eq!(previous_content.as_ref().unwrap(), &content);
            assert_eq!(commands[2]["published_revision"], revision);
            assert_eq!(
                commands[2]["source_commit"],
                artifact.manifest()["capture_token"]
            );
            assert_eq!(
                artifact.manifest()["command_cause"]["private_request_id"],
                6
            );
            assert_eq!(
                artifact.manifest()["command_cause"]["authority_gate"],
                "core-ledger-admitted"
            );
        }
        assert!(receiver.resources_ready().unwrap());
        assert!(receiver.presented_revision().unwrap().is_none());
    }
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
}
