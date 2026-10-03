//! Retained actual GTK IM -> wire composition -> native causal captures.
//! The independent packet/stage audit runs separately without native GTK.
use rui_producer_sdk::CaptureArtifact;
use rui_protocol_core::model::{Record, TextRange};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::Path};

fn inventory(root: &Path, directory: &Path, names: &mut BTreeSet<String>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        assert!(!path.is_symlink());
        if path.is_dir() {
            inventory(root, &path, names);
        } else {
            assert!(matches!(
                path.extension().unwrap().to_str().unwrap(),
                "json" | "jsonl" | "bin" | "rvp"
            ));
            names.insert(path.strip_prefix(root).unwrap().to_str().unwrap().into());
        }
    }
}

#[test]
fn native_desktop_composition_report_binds_exact_vector_closure_and_causal_text() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gtk");
    let report: Value = serde_json::from_slice(
        &fs::read(base.join("desktop-composition-interaction.json")).unwrap(),
    )
    .unwrap();
    let root = base.join("desktop-composition-golden");
    let hashes = report["retained_exact_trace_sha256"].as_object().unwrap();
    let mut files = BTreeSet::new();
    inventory(&root, &root, &mut files);
    assert_eq!(files, hashes.keys().cloned().collect());
    assert_eq!(files.len(), 50);
    let mut bytes = 0;
    for (name, hash) in hashes {
        let body = fs::read(root.join(name)).unwrap();
        bytes += body.len();
        assert_eq!(
            format!("{:x}", Sha256::digest(body)),
            hash.as_str().unwrap()
        );
    }
    assert_eq!(report["retained_bytes"], bytes);
    assert!(bytes < 4 << 20);
    assert_eq!(report["host_memory_after_disposal"], 0);
    assert_eq!(
        report["selected_clock_profile"],
        "text.clock.gtk-editable4224.v1"
    );
    assert_eq!(
        report["native_callbacks"],
        serde_json::json!({"begins":5,"updates":12,"commits":4,"cancels":1})
    );
    assert_eq!(report["preedit_privacy"]["document_unchanged"], true);
    assert_eq!(
        report["preedit_privacy"]["observer_private_preedit_visible"],
        false
    );
    assert_eq!(report["escape"]["document_unchanged"], true);
    assert_eq!(report["priority_reset"]["has_control"], false);
    for key in [
        "initial",
        "focused",
        "ascii",
        "unicode",
        "rapid_native_commits",
    ] {
        let frames = report[key].as_array().unwrap();
        assert_eq!(frames.len(), 2);
        for frame in frames {
            assert_eq!(
                frame["direct_source_records_sha256"],
                frame["mirror_records_sha256"]
            );
            assert_eq!(
                frame["direct_source_rgba_sha256"],
                frame["actual_after_paint_rgba_sha256"]
            );
            assert!(
                frame["frame_clock"]["after_paint_time_us"]
                    .as_u64()
                    .unwrap()
                    > 0
            );
        }
    }
    let vector = root.join("source-vector");
    let first = CaptureArtifact::read(vector.join("whole-0001.json")).unwrap();
    let mut receiver = first.receiver().unwrap();
    let entry = first.manifest()["semantics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["identity"] == "modal-entry")
        .unwrap();
    let text_id = entry["text"]["id"].as_u64().unwrap();
    let comparisons = report["native_reference_comparison"]["captures"]
        .as_array()
        .unwrap();
    assert_eq!(comparisons.len(), 5);
    let mut glyphs = 0;
    for (revision, content, caret, content_clock, record_clock) in [
        (1, "Modal scalar text: 𝄞 é שלום", None, 1, 1),
        (2, "Modal scalar text: 𝄞 é שלום", None, 1, 1),
        (3, "a", Some(1), 3, 4),
        (4, "aé", Some(2), 5, 7),
        (6, "aéxy", Some(4), 9, 13),
    ] {
        let name = format!("whole-{revision:04}.json");
        let artifact = CaptureArtifact::read(vector.join(&name)).unwrap();
        artifact.apply_trusted_fixture(&mut receiver).unwrap();
        artifact.verify_vector_audit(&receiver).unwrap();
        let state = receiver.state().unwrap();
        assert_eq!(state.revision, revision);
        let Record::Text(text) = &state.records[&text_id] else {
            panic!("native TextModel missing")
        };
        assert_eq!(text.content.as_deref(), Some(content));
        assert_eq!(
            (text.text_version, text.version),
            (content_clock, record_clock)
        );
        if let Some(caret) = caret {
            assert_eq!(
                text.selection,
                Some(TextRange {
                    anchor: caret,
                    focus: caret
                })
            );
            let commit = report["wire_composition_trace"]["commits"]
                .as_array()
                .unwrap()
                .iter()
                .find(|commit| commit["revision"] == revision)
                .unwrap();
            let cause = commit["source_commit"].as_str().unwrap().as_bytes();
            let session = state
                .records
                .values()
                .find_map(|record| match record {
                    Record::Session(session)
                        if session
                            .focus
                            .is_some_and(|focus| focus.id == entry["id"].as_u64().unwrap()) =>
                    {
                        Some(session)
                    }
                    _ => None,
                })
                .unwrap();
            let Record::Surface(surface) = &state.records[&session.focus_surface.unwrap().id]
            else {
                panic!("focus Surface missing")
            };
            assert!(surface
                .source_capture_tokens
                .iter()
                .any(|token| token == cause));
        }
        let comparison = comparisons
            .iter()
            .find(|comparison| comparison["capture"] == name)
            .unwrap();
        assert_eq!(
            comparison["envelope_sha256"],
            artifact.manifest()["envelope_sha256"]
        );
        assert_eq!(
            comparison["local_reference_sha256"],
            artifact.manifest()["local_reference"]["sha256"]
        );
        assert_eq!(comparison["pixels"]["passed"], true);
        assert_eq!(comparison["pixels"]["interior_bad_pixels"], 0);
        glyphs += comparison["geometry"]["glyphs_checked"].as_u64().unwrap();
        // This replay validates artifacts; only the native Canvas callback in
        // the hash-bound report supplies actual presentation evidence.
        assert_eq!(receiver.presented_revision().unwrap(), None);
    }
    assert_eq!(glyphs, 935);
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
}
