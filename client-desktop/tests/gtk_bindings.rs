//! Actual source captures back the mirror's generic pointer discovery.
use rui_client_desktop::{
    interaction::{hit_test, InteractionError},
    Viewport,
};
use rui_producer_sdk::CaptureArtifact;
use rui_protocol_core::model::{Point, Record};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::PathBuf};

#[test]
fn actual_mirror_frames_bind_source_records_pixels_and_popup_background_ownership() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/gtk");
    let root = base.join("mirror-golden");
    let report: Value =
        serde_json::from_slice(&fs::read(base.join("mirror-interaction.json")).unwrap()).unwrap();
    let hashes = report["retained_exact_trace_sha256"].as_object().unwrap();
    let actual: BTreeSet<_> = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(actual, hashes.keys().cloned().collect());
    for (name, hash) in hashes {
        assert!(matches!(
            root.join(name).extension().unwrap().to_str(),
            Some("json" | "jsonl" | "bin" | "rvp")
        ));
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(root.join(name)).unwrap())),
            hash.as_str().unwrap()
        );
    }
    assert_eq!(report["host_memory_after_disposal"], 0);
    assert_eq!(report["native_dependencies"]["gtk4"], "4.22.4");
    for proof in report["initial"]
        .as_array()
        .unwrap()
        .iter()
        .chain(report["after_native_activate"].as_array().unwrap())
    {
        assert_eq!(
            proof["direct_source_records_sha256"],
            proof["mirror_records_sha256"]
        );
        assert_eq!(
            proof["direct_source_rgba_sha256"],
            proof["actual_after_paint_rgba_sha256"]
        );
        assert_eq!(proof["viewport"]["width"], 1024);
        assert_eq!(proof["viewport"]["height"], 640);
        // GDK's frame time may be extrapolated ahead of wall-clock time;
        // the real after-paint observation records both clocks separately.
        assert!(
            proof["frame_clock"]["after_paint_time_us"]
                .as_i64()
                .unwrap()
                > 0
        );
        assert!(proof["frame_clock"]["frame_time_us"].as_i64().unwrap() > 0);
    }
    for comparison in report["native_reference_comparison"]["captures"]
        .as_array()
        .unwrap()
    {
        assert_eq!(comparison["pixels"]["interior_bad_pixels"], 0);
        assert_eq!(comparison["pixels"]["passed"], true);
        assert!(comparison["geometry"]["glyphs_checked"].as_u64().unwrap() >= 100);
    }
    let first = CaptureArtifact::read(root.join("whole-0001.json")).unwrap();
    let mut receiver = first.receiver().unwrap();
    for revision in 1..=2 {
        let artifact =
            CaptureArtifact::read(root.join(format!("whole-{revision:04}.json"))).unwrap();
        artifact.apply_trusted_fixture(&mut receiver).unwrap();
        artifact.verify_vector_audit(&receiver).unwrap();
        assert!(receiver.presented_revision().unwrap().is_none());
    }
    let state = receiver.state().unwrap();
    let point = Point { x: 100.0, y: 370.0 };
    // This actual popup background overlaps the native main menu button. Its
    // native content owns input even though no semantic widget is bound there.
    assert_eq!(
        hit_test(state, state.revision, Viewport::new(720, 600), point),
        Err(InteractionError::NoTarget)
    );
    let mut regression = state.clone();
    let menu = regression
        .records
        .values_mut()
        .find_map(|record| match record {
            Record::Surface(surface)
                if surface.desktop_rect.x == 40.0 && surface.desktop_rect.y == 330.0 =>
            {
                Some(surface)
            }
            _ => None,
        })
        .unwrap();
    menu.input_region.clear();
    let incorrectly_exposed = hit_test(
        &regression,
        regression.revision,
        Viewport::new(720, 600),
        point,
    )
    .unwrap();
    let Record::Semantic(node) = &regression.records[&incorrectly_exposed.semantic] else {
        panic!("semantic")
    };
    assert_eq!(node.label.as_deref(), Some("Ordinary GTK menu"));
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
}
