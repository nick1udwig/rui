//! Exact core-admitted native GTK Unicode capture/effect evidence.
use rui_producer_sdk::CaptureArtifact;
use rui_protocol_core::model::{Record, TextRange};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::PathBuf};

#[test]
fn native_supplementary_combining_rtl_ranges_content_and_exact_retries() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gtk");
    let report: Value =
        serde_json::from_slice(&fs::read(base.join("unicode-interaction.json")).unwrap()).unwrap();
    let root = base.join("unicode-golden/unicode");
    let actual: BTreeSet<_> = fs::read_dir(&root)
        .unwrap()
        .map(|e| format!("unicode/{}", e.unwrap().file_name().to_string_lossy()))
        .collect();
    let declared: BTreeSet<_> = report["retained_trace_sha256"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(actual, declared);
    for (name, digest) in report["retained_trace_sha256"].as_object().unwrap() {
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(fs::read(base.join("unicode-golden").join(name)).unwrap())
            ),
            digest.as_str().unwrap()
        );
    }
    assert_eq!(report["unicode"]["native_dispatches"], 8);
    assert_eq!(report["unicode"]["native_callbacks"], 8);
    let mut paths = fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect::<Vec<_>>();
    paths.sort();
    assert_eq!(paths.len(), 9);
    let first = CaptureArtifact::read(&paths[0]).unwrap();
    let semantic = first.manifest()["semantics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["identity"] == "entry")
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let mut receiver = first.receiver().unwrap();
    let mut content_clock = None;
    let mut previous_record_clock = 0;
    let mut tokens = BTreeSet::new();
    for path in paths {
        let artifact = CaptureArtifact::read(&path).unwrap();
        let bytes = fs::read(path.with_extension("bin")).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            artifact.manifest()["envelope_sha256"]
        );
        artifact.apply_trusted_fixture(&mut receiver).unwrap();
        artifact.verify_vector_audit(&receiver).unwrap();
        tokens.insert(
            artifact.manifest()["capture_token"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
        let state = receiver.state().unwrap();
        let Record::Semantic(node) = &state.records[&semantic] else {
            panic!("entry")
        };
        let Record::Text(text) = &state.records[&node.text.unwrap().id] else {
            panic!("text")
        };
        assert!(text.version >= previous_record_clock);
        previous_record_clock = text.version;
        let (content, range) = match state.revision {
            3 => ("A𝄞e\u{301} אבגZ", (9, 9)),
            4 => ("A𝄞e\u{301} אבגZ", (1, 2)),
            5 => ("A𝄞e\u{301} אבגZ", (2, 3)),
            6 => ("A𝄞e\u{301} אבגZ", (8, 5)),
            7 => ("A𐐷e\u{301} אבגZ", (2, 2)),
            8 => ("A𐐷e\u{308} אבגZ", (4, 4)),
            9 => ("A𐐷e\u{308} שלוםZ", (9, 9)),
            _ => continue,
        };
        assert_eq!(text.content.as_deref(), Some(content));
        assert_eq!(
            text.selection,
            Some(TextRange {
                anchor: range.0,
                focus: range.1
            })
        );
        if state.revision == 3 {
            content_clock = Some(text.text_version);
        }
        if (4..=6).contains(&state.revision) {
            assert_eq!(Some(text.text_version), content_clock);
        }
        assert!(receiver.presented_revision().unwrap().is_none());
    }
    let commands = report["unicode"]["commands"].as_array().unwrap();
    assert_eq!(commands.len(), 10);
    let mut exact = None;
    for entry in commands {
        let raw = entry["exact_semantic_wire_hex"].as_str().unwrap();
        let bytes: Vec<_> = raw
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
            .collect();
        rui_protocol_core::commands::decode_command(&bytes).unwrap();
        if entry["case"] == "supplementary-combining-rtl-native-replace" {
            exact = Some(bytes.clone());
        }
        if entry["case"] == "exact-mixed-text-retry" {
            assert_eq!(Some(bytes), exact);
        } else if entry["status"] == "Applied" {
            assert!(tokens.contains(entry["source_commit"].as_str().unwrap()));
        }
        if entry["case"] == "mixed-text-stale-content-clock" {
            assert_eq!(entry["status"], "Rejected");
            assert_eq!(entry["rejection"], "StaleText");
        }
    }
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
}
