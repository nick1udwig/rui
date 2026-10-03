use rui_producer_sdk::CaptureArtifact;
use rui_protocol_core::model::{Primitive, Record, VisualCoverage};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gtk")
}

#[test]
fn actual_photo_texture_cairo_blur_shader_sources_are_named_partial_vector_only() {
    let base = fixture();
    let report: Value =
        serde_json::from_slice(&fs::read(base.join("negative-report.json")).unwrap()).unwrap();
    let root = base.join("negative-golden");
    let actual_files: std::collections::BTreeSet<_> = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    let declared_files: std::collections::BTreeSet<_> = report["retained_trace_sha256"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(actual_files, declared_files);
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(fs::read(base.join("assets/PIA19400.jpg")).unwrap())
        ),
        report["source_photo"]["sha256"]
    );
    for (name, hash) in report["retained_trace_sha256"].as_object().unwrap() {
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(root.join(name)).unwrap())),
            hash.as_str().unwrap()
        );
    }
    assert_eq!(
        report["fallback_probe"]["forbidden_export_calls"],
        serde_json::json!({})
    );
    assert_eq!(
        report["fallback_probe"]["negative_controls_tripped"],
        serde_json::json!({"ImageSurface":1,"Texture.download":1})
    );
    assert_eq!(
        report["fallback_probe"]["allowed_outside_export_calls"]["RenderNode.draw"],
        6
    );
    let mut receiver = None;
    for case in report["cases"].as_array().unwrap() {
        let artifact =
            CaptureArtifact::read(root.join(format!("{}.json", case["label"].as_str().unwrap())))
                .unwrap();
        let bytes =
            fs::read(root.join(format!("{}.bin", case["label"].as_str().unwrap()))).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            artifact.manifest()["envelope_sha256"]
        );
        assert_eq!(
            case["local_reference"]["sha256"],
            artifact.manifest()["local_reference"]["sha256"]
        );
        if case["case"] == "original-photo" {
            assert!(
                case["local_reference"]["distinct_region_colors"]
                    .as_u64()
                    .unwrap()
                    > 256
            );
        }
        let receiver = receiver.get_or_insert_with(|| artifact.receiver().unwrap());
        artifact.apply_trusted_fixture(receiver).unwrap();
        let audit = artifact.verify_vector_audit(receiver).unwrap();
        assert!(audit.observed_counter_schema);
        assert!(receiver.presented_revision().unwrap().is_none());
        let expected = case["case"] == "native-vector-control";
        assert_eq!(audit.resource_bodies, usize::from(expected));
        assert_eq!(audit.unsupported_subtrees, usize::from(!expected));
        if !expected {
            assert_eq!(
                audit.primitive_counts,
                [("Group".into(), 2), ("SolidRect".into(), 1)].into()
            );
            assert!(receiver.state().unwrap().records.values().any(
                |r| matches!(r,Record::Surface(s) if s.visual_coverage==VisualCoverage::Partial)
            ));
            assert!(receiver.state().unwrap().records.values().all(|r|!matches!(r,Record::Scene(s) if matches!(s.primitive,Primitive::FillPath{..}|Primitive::StrokePath{..}|Primitive::GlyphRun{..}))));
        }
    }
    let receiver = receiver.as_mut().unwrap();
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
}

#[test]
fn diagnostic_audit_rejects_geometry_gap_and_origin_substitution() {
    let root = fixture().join("negative-golden");
    for (label, mutation) in [
        ("negative-content-0001", 0),
        ("negative-content-0001", 1),
        ("negative-content-0006", 2),
        ("negative-content-0001", 3),
        ("negative-content-0001", 4),
        ("negative-content-0001", 5),
        ("negative-content-0001", 6),
    ] {
        let temp = std::env::temp_dir().join(format!(
            "rui-negative-audit-{}-{mutation}",
            std::process::id()
        ));
        fs::create_dir(&temp).unwrap();
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(root.join(format!("{label}.json"))).unwrap()).unwrap();
        for entry in fs::read_dir(&root).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "rvp") {
                fs::copy(&path, temp.join(path.file_name().unwrap())).unwrap();
            }
        }
        fs::copy(
            root.join(format!("{label}.bin")),
            temp.join(format!("{label}.bin")),
        )
        .unwrap();
        match mutation {
            0 => manifest["scene"][0]["primitive"]["rect"][2] = 199.into(),
            1 => manifest["gaps"] = serde_json::json!([]),
            2 => {
                let digest = manifest["resources"][0].as_str().unwrap().to_owned();
                manifest["geometry_resource_origins"][&digest][0]["origin"] =
                    "pixel-to-vector".into();
            }
            3 => {
                manifest.as_object_mut().unwrap().remove("scene");
            }
            4 => manifest["protocol_handoff"]["resource_kind_counts"]["1"] = 1.into(),
            5 => manifest["gaps"][0]["path"] = "root/0".into(),
            _ => manifest["gaps"][0]["bounds"][2] = 159.into(),
        }
        let path = temp.join(format!("{label}.json"));
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let artifact = CaptureArtifact::read(path).unwrap();
        let mut receiver = artifact.receiver().unwrap();
        artifact.apply_trusted_fixture(&mut receiver).unwrap();
        assert!(artifact.verify_vector_audit(&receiver).is_err());
        if mutation == 3 {
            let result = std::process::Command::new(env!("CARGO_BIN_EXE_replay_capture"))
                .arg(&temp)
                .output()
                .unwrap();
            assert!(!result.status.success());
            assert!(!String::from_utf8_lossy(&result.stdout).contains("historical-glyph-only"));
        }
        receiver.close();
        fs::remove_dir_all(temp).unwrap();
    }
}
