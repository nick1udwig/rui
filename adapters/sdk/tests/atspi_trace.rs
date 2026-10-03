//! Exact observed publications from two real applications on a private AT-SPI bus.
use rui_protocol_core::{
    model::{
        CaptureConsistency, Completeness, Limits, Record, Replica, SemanticCoverage, VisualCoverage,
    },
    wire,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::PathBuf};

#[test]
fn observed_gap_race_stall_restart_and_ambiguous_surface_identity() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/atspi/golden");
    let report: Value =
        serde_json::from_slice(&fs::read(root.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["classification"], "OBSERVED");
    assert_eq!(report["native_surface_association"], "unresolved");
    assert!(report["input_modes"].as_array().unwrap().is_empty());
    for value in report["ambiguous_windows"].as_object().unwrap().values() {
        assert_eq!(value, true);
    }
    assert_eq!(
        report["stalled_source"]["previous_publication_unchanged"],
        true
    );
    assert!(
        report["stalled_source"]["owner_poll_iterations"]
            .as_u64()
            .unwrap()
            >= 20
    );
    let mut expected: BTreeSet<_> = report["files"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    expected.insert("report.json".into());
    assert_eq!(
        expected,
        fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_str().unwrap().to_owned())
            .collect()
    );
    let mut replica = Replica::new(Limits::default());
    let mut binding = None;
    let mut initial_ids = BTreeSet::new();
    let mut surviving_ids = BTreeSet::new();
    for capture in report["captures"].as_array().unwrap() {
        let name = capture["name"].as_str().unwrap();
        let file = format!("{name}.bin");
        let bytes = fs::read(root.join(&file)).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            report["files"][&file]
        );
        let envelope = wire::decode_envelope(&bytes, &Limits::default()).unwrap();
        let scope = (envelope.session_id, envelope.epoch, envelope.view_id);
        assert_eq!(binding.get_or_insert_with(|| scope.clone()), &scope);
        replica
            .apply_snapshot(&envelope.payload, &BTreeSet::new())
            .unwrap();
        let state = replica.state();
        assert_eq!(state.revision, capture["revision"].as_u64().unwrap());
        let mut surfaces = 0;
        for record in state.records.values() {
            match record {
                Record::Surface(surface) => {
                    surfaces += 1;
                    assert_eq!(surface.capture_consistency, CaptureConsistency::Observed);
                    assert_eq!(surface.visual_coverage, VisualCoverage::Unsupported);
                    assert_eq!(surface.semantic_coverage, SemanticCoverage::Partial);
                    assert!(surface.scene_root.is_none());
                    assert!(surface.routing_token.is_empty() && surface.input_region.is_empty());
                    assert_eq!(
                        surface.unsupported_features,
                        ["native-surface-association-unresolved"]
                    );
                    assert_eq!(surface.source_capture_tokens.len(), 2);
                }
                Record::Semantic(node) => {
                    assert_eq!(node.completeness, Completeness::Incomplete);
                    assert!(node.actions.is_empty() && node.text.is_none() && node.value.is_none());
                }
                Record::Session(_) => {}
                _ => panic!("unexpected visual, text, input or resource-bearing object"),
            }
        }
        assert_eq!(surfaces, if name == "source-exited" { 1 } else { 2 });
        let ids: BTreeSet<_> = state.records.keys().copied().collect();
        if name == "initial" {
            initial_ids = ids.clone();
        }
        if name == "event-gap" {
            assert!(capture["dropped_events"].as_u64().unwrap() > 0);
            assert!(state.records.values().any(
                |r| matches!(r, Record::Semantic(n) if n.label.as_deref() == Some("Replacement 1"))
            ));
        }
        if name == "crawl-race" {
            assert!(capture["reconciliations"].as_u64().unwrap() >= 1);
            assert!(state.records.values().any(
                |r| matches!(r, Record::Semantic(n) if n.label.as_deref() == Some("Replacement 2"))
            ));
        }
        if name == "source-exited" {
            surviving_ids = ids.clone();
            assert_eq!(surviving_ids.len(), 28);
            assert!(surviving_ids.is_subset(&initial_ids));
            assert!(initial_ids
                .difference(&surviving_ids)
                .all(|id| state.retired.contains(id)));
        }
        if name == "source-restarted" {
            assert!(surviving_ids.is_subset(&ids));
            assert_eq!(
                ids.intersection(&initial_ids)
                    .copied()
                    .collect::<BTreeSet<_>>(),
                surviving_ids
            );
            assert_eq!(ids.len(), 55);
        }
        assert!(capture["calls"].as_u64().unwrap() <= 1024);
        assert!(capture["worker_peak_rss_kib"].as_u64().unwrap() < 256 * 1024);
    }
    assert_eq!(replica.state().revision, 6);
}
