//! Native transitions within one frame remain visible to source context guards.
use rui_producer_sdk::CaptureArtifact;
use rui_protocol_core::model::Record;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::PathBuf};

fn base() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gtk")
}
#[test]
fn native_focus_and_selection_away_back_cannot_restore_a_stale_context() {
    let report: Value =
        serde_json::from_slice(&fs::read(base().join("native-context-interaction.json")).unwrap())
            .unwrap();
    let root = base().join("native-context-golden");
    let evidence = &report["native_context"];
    let actual: BTreeSet<_> = fs::read_dir(root.join("native-context"))
        .unwrap()
        .map(|e| {
            format!(
                "native-context/{}",
                e.unwrap().file_name().to_string_lossy()
            )
        })
        .collect();
    let hashes = report["retained_trace_sha256"].as_object().unwrap();
    assert_eq!(actual, hashes.keys().cloned().collect());
    assert_eq!(actual.len(), 48);
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
    let commands = evidence["commands"].as_array().unwrap();
    assert_eq!(commands.len(), 4);
    assert_eq!(commands[1]["status"], "Failed");
    assert_eq!(
        commands[1]["native_dispatches"],
        commands[0]["native_dispatches"]
    );
    assert_eq!(
        commands[2]["exact_semantic_wire_hex"],
        commands[1]["exact_semantic_wire_hex"]
    );
    assert_eq!(commands[2]["retained_result_equal"], true);
    assert_eq!(commands[2]["received_native_counters_unchanged"], true);
    assert_eq!(evidence["native_dispatches"], 2);
    let directory = root.join("native-context");
    let first = CaptureArtifact::read(directory.join("snapshot-01-000001.json")).unwrap();
    let mut receiver = first.receiver().unwrap();
    let mut before_focus_cycle = None;
    let mut before_selection_cycle = None;
    for revision in 1..=5 {
        let artifact =
            CaptureArtifact::read(directory.join(format!("snapshot-01-{revision:06}.json")))
                .unwrap();
        artifact.apply_trusted_fixture(&mut receiver).unwrap();
        artifact.verify_vector_audit(&receiver).unwrap();
        assert_eq!(
            artifact.manifest()["native_context_profile"],
            "gtk-native-focus-context.v1"
        );
        assert_eq!(
            artifact.manifest()["source_boundary_facts"]["focus_generation"],
            artifact.manifest()["session"]["focus_generation"]
        );
        let state = receiver.state().unwrap();
        let Record::Session(session) = state
            .records
            .values()
            .find(|r| matches!(r, Record::Session(_)))
            .unwrap()
        else {
            panic!("session")
        };
        let Record::Semantic(entry) = state.records.values().find(|r| matches!(r, Record::Semantic(node) if node.label.as_deref() == Some("Modal entry"))).unwrap() else {
            panic!("entry")
        };
        if revision >= 2 {
            assert_eq!(session.focus.unwrap().id, entry.id);
        }
        let Record::Text(text) = &state.records[&entry.text.unwrap().id] else {
            panic!("text")
        };
        if revision == 2 {
            before_focus_cycle = Some((
                session.focus,
                session.focus_surface,
                session.focus_generation,
            ));
        }
        if revision == 3 {
            let (target, surface, clock) = before_focus_cycle.unwrap();
            assert_eq!((session.focus, session.focus_surface), (target, surface));
            assert_eq!(session.focus_generation, clock + 2);
            assert_eq!(
                evidence["focus_generation_after_native_cycle"],
                session.focus_generation
            );
        }
        if revision == 4 {
            before_selection_cycle = Some(text.clone());
        }
        if revision == 5 {
            let previous = before_selection_cycle.as_ref().unwrap();
            assert_eq!(previous.selection, text.selection);
            assert_eq!(previous.content, text.content);
            assert_eq!(previous.text_version, text.text_version);
            assert_eq!(text.version, previous.version + 2);
            assert_eq!(evidence["text_record_after_selection_cycle"], text.version);
        }
        assert!(receiver.resources_ready().unwrap());
        assert!(receiver.presented_revision().unwrap().is_none());
    }
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
}

#[test]
fn source_boundary_focus_clock_substitution_fails_before_presentation() {
    let root = base().join("native-context-golden/native-context");
    let original = root.join("snapshot-01-000003.json");
    let artifact = CaptureArtifact::read(&original).unwrap();
    let mut receiver = artifact.receiver().unwrap();
    artifact.apply_trusted_fixture(&mut receiver).unwrap();
    let temporary = std::env::temp_dir().join(format!(
        "rui-native-context-negative-{}",
        std::process::id()
    ));
    fs::create_dir(&temporary).unwrap();
    let path = temporary.join("capture.json");
    let mut manifest = artifact.manifest().clone();
    manifest["source_boundary_facts"]["focus_generation"] = serde_json::json!(1);
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    fs::copy(original.with_extension("bin"), path.with_extension("bin")).unwrap();
    let forged = CaptureArtifact::read(&path).unwrap();
    assert!(forged.verify_vector_audit(&receiver).is_err());
    assert!(receiver.presented_revision().unwrap().is_none());
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
    fs::remove_dir_all(temporary).unwrap();
}
