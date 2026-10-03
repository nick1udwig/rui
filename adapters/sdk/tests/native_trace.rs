//! Retained actual admitted GTK source publications, not a synthetic reducer recipe.
use prost::Message;
use rui_producer_sdk::CaptureArtifact;
use rui_protocol_core::model::{Record, SemanticNode, TextRange};
use rui_protocol_core::wire::proto as pb;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

fn semantic_id(artifact: &CaptureArtifact, key: &str) -> u64 {
    artifact.manifest()["semantics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["identity"] == key)
        .unwrap()["id"]
        .as_u64()
        .unwrap()
}
fn semantic(state: &rui_protocol_core::model::State, id: u64) -> &SemanticNode {
    let Record::Semantic(node) = &state.records[&id] else {
        panic!("semantic")
    };
    node
}

#[test]
fn actual_native_trace_integrity_clocks_effects_and_partial_coverage_replay() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gtk");
    let root = fixture.join("native-golden");
    let report: Value =
        serde_json::from_slice(&fs::read(fixture.join("native-interaction.json")).unwrap())
            .unwrap();
    let actual_files: std::collections::BTreeSet<_> = [
        "semantic",
        "escaped-text",
        "pipe-loss",
        "pipe-stall",
        "uncorrelated-capture",
        "delayed-expiry",
    ]
    .into_iter()
    .flat_map(|scenario| {
        fs::read_dir(root.join(scenario))
            .unwrap()
            .map(move |entry| {
                format!(
                    "{scenario}/{}",
                    entry.unwrap().file_name().to_string_lossy()
                )
            })
    })
    .collect();
    let declared_files: std::collections::BTreeSet<_> = report["retained_trace_sha256"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(actual_files, declared_files);
    for (name, digest) in report["retained_trace_sha256"].as_object().unwrap() {
        let actual = format!("{:x}", Sha256::digest(fs::read(root.join(name)).unwrap()));
        assert_eq!(
            actual,
            digest.as_str().unwrap(),
            "exact retained file {name}"
        );
    }
    assert_eq!(report["semantic"]["native_dispatches"], 32);
    assert_eq!(report["semantic"]["native_callbacks"], 37);
    for name in ["pipe_loss", "pipe_stall", "uncorrelated_capture"] {
        assert_eq!(report[name]["commands"][0]["status"], "OutcomeUnknown");
        assert_eq!(report[name]["commands"][1]["case"], "exact-unknown-retry");
        assert_eq!(report[name]["commands"][2]["status"], "Rejected");
        assert_eq!(
            report[name]["local_negative_control_receipt"]["native_dispatches"],
            1
        );
    }
    assert_eq!(report["delayed_expiry"]["native_dispatches"], 0);
    let mut captures = 0;
    let mut source_tokens = std::collections::BTreeSet::new();
    for scenario in [
        "semantic",
        "escaped-text",
        "pipe-loss",
        "pipe-stall",
        "uncorrelated-capture",
        "delayed-expiry",
    ] {
        let mut files = fs::read_dir(root.join(scenario))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect::<Vec<_>>();
        files.sort();
        let first = CaptureArtifact::read(&files[0]).unwrap();
        let entry = semantic_id(&first, "entry");
        let mut receiver = first.receiver().unwrap();
        let mut previous_text: Option<rui_protocol_core::model::TextModel> = None;
        let mut saw_selection_only = false;
        let mut saw_stable_text = false;
        for path in files {
            let artifact = CaptureArtifact::read(&path).unwrap();
            source_tokens.insert(
                artifact.manifest()["capture_token"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
            let bytes = fs::read(path.with_extension("bin")).unwrap();
            assert_eq!(
                format!("{:x}", Sha256::digest(&bytes)),
                artifact.manifest()["envelope_sha256"].as_str().unwrap()
            );
            artifact.apply_trusted_fixture(&mut receiver).unwrap();
            artifact.verify_native_glyph_evidence(&receiver).unwrap();
            artifact.verify_vector_audit(&receiver).unwrap();
            assert!(receiver.resources_ready().unwrap());
            assert!(receiver.presented_revision().unwrap().is_none());
            if artifact.manifest()["destroyed"] == true {
                assert!(receiver.state().unwrap().records.values().all(|record| matches!(record, Record::Session(session) if session.focus.is_none() && session.focus_surface.is_none())));
                assert!(receiver.state().unwrap().resource_refs().is_empty());
            } else {
                let state = receiver.state().unwrap();
                let text_id = semantic(state, entry).text.unwrap().id;
                let Record::Text(text) = &state.records[&text_id] else {
                    panic!("native text")
                };
                if let Some(previous) = &previous_text {
                    if previous == text {
                        saw_stable_text = true;
                    } else if previous.content == text.content
                        && previous.selection != text.selection
                    {
                        assert!(text.version > previous.version);
                        assert_eq!(text.text_version, previous.text_version);
                        saw_selection_only = true;
                    }
                }
                previous_text = Some(text.clone());
                let content = text.content.as_deref().unwrap();
                match content {
                    "AéΩZ一" => assert_eq!(
                        text.selection,
                        Some(TextRange {
                            anchor: 5,
                            focus: 5
                        })
                    ),
                    "AéΩZ一二"
                        if text.selection
                            == Some(TextRange {
                                anchor: 3,
                                focus: 1,
                            }) =>
                    {
                        assert!(text.text_version > 1)
                    }
                    "AéΩZ一二" => assert_eq!(
                        text.selection,
                        Some(TextRange {
                            anchor: 6,
                            focus: 6
                        })
                    ),
                    _ => {}
                }
                if content.len() == 16 << 10 {
                    assert!(content.chars().all(|c| c == '\u{1}'));
                    let envelope = pb::Envelope::decode(bytes.as_slice()).unwrap();
                    let snapshot = pb::Snapshot::decode(envelope.payload.as_slice()).unwrap();
                    assert!(snapshot.records.iter().any(|r|matches!(r.record.as_ref(),Some(pb::record::Record::Surface(s)) if s.visual_coverage == 2 && s.unsupported_features.contains(&"glyph-count-ceiling".into()))));
                }
            }
            captures += 1;
        }
        if scenario == "semantic" {
            assert!(saw_selection_only && saw_stable_text);
        }
        receiver.close();
        assert_eq!(receiver.memory_used(), 0);
    }
    assert!(captures >= 40);
    let mut commands = 0;
    let mut exact_commands = std::collections::BTreeMap::new();
    for scenario in [
        "semantic",
        "escaped_text",
        "pipe_loss",
        "pipe_stall",
        "uncorrelated_capture",
        "delayed_expiry",
    ] {
        for entry in report[scenario]["commands"].as_array().unwrap() {
            let Some(exact) = entry["exact_semantic_wire_hex"].as_str() else {
                continue;
            };
            assert!(exact.len() <= 128 << 10);
            let bytes: Vec<_> = exact
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
                .collect();
            let command = rui_protocol_core::commands::decode_command(&bytes).unwrap();
            let key = (
                command.client_instance_id,
                command.view_id,
                command.command_seq,
            );
            if let Some(action) = entry["action"].as_str() {
                assert_eq!(command.action_id, action);
                assert_eq!(command.target_id, entry["target_id"].as_u64().unwrap());
            } else {
                assert_eq!(entry["retained_result_equal"], true);
                assert_eq!(exact_commands.get(&key), Some(&bytes));
            }
            exact_commands.insert(key, bytes);
            assert_eq!(command.command_seq, entry["command_seq"].as_u64().unwrap());
            if entry["status"] == "Applied" && entry["action"].is_string() {
                assert!(source_tokens.contains(entry["source_commit"].as_str().unwrap()));
                assert!(
                    entry["published_revision"].as_u64().unwrap()
                        > entry["observed_view_revision"].as_u64().unwrap()
                );
            }
            commands += 1;
        }
    }
    assert!(commands >= 40);
}
