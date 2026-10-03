//! Source-owned native bounds survive exact replay and fail closed on changes.
use prost::Message;
use rui_producer_sdk::CaptureArtifact;
use rui_protocol_core::{model::Record, wire::proto as pb};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::PathBuf};

fn base() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gtk")
}

#[test]
fn actual_native_move_hide_remap_disable_and_retirement_preserve_safe_bindings() {
    let report: Value =
        serde_json::from_slice(&fs::read(base().join("binding-interaction.json")).unwrap())
            .unwrap();
    let directory = base().join("binding-golden");
    let actual: BTreeSet<_> = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let hashes = report["retained_trace_sha256"].as_object().unwrap();
    assert_eq!(actual, hashes.keys().cloned().collect());
    for (name, hash) in hashes {
        assert!(matches!(
            directory.join(name).extension().unwrap().to_str(),
            Some("bin" | "json" | "rvp")
        ));
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(fs::read(directory.join(name)).unwrap())
            ),
            hash.as_str().unwrap()
        );
    }
    let frames = report["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 6);
    let first =
        CaptureArtifact::read(directory.join(frames[0]["capture"].as_str().unwrap())).unwrap();
    let mut receiver = first.receiver().unwrap();
    let binding = frames[0]["binding_id"].as_u64().unwrap();
    let mut clock = 0;
    for frame in frames {
        let artifact =
            CaptureArtifact::read(directory.join(frame["capture"].as_str().unwrap())).unwrap();
        artifact.apply_trusted_fixture(&mut receiver).unwrap();
        artifact.verify_vector_audit(&receiver).unwrap();
        let state = receiver.state().unwrap();
        assert_eq!(frame["counter_rotated_clip_denied"], true);
        // Native child and its clipping ancestor have cancelling rotations;
        // the source still denies the unsafe clipping-ancestor geometry.
        let counter = artifact.manifest()["semantics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["identity"] == "counter")
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        assert!(state.records.values().any(|record| matches!(record,Record::Binding(b) if b.semantics.iter().any(|node|node.id==counter) && b.interaction_region.is_none())));
        if frame["stage"] == "removed" {
            assert!(!state.records.contains_key(&binding));
            assert!(state.retired.contains(&binding));
        } else {
            let Record::Binding(current) = &state.records[&binding] else {
                panic!("binding")
            };
            assert!(current.version > clock);
            clock = current.version;
            match frame["stage"].as_str().unwrap() {
                "initial" | "remapped" => assert_eq!(current.interaction_region.unwrap().x, 10.0),
                "moved" => assert_eq!(current.interaction_region.unwrap().x, 40.0),
                "hidden" | "disabled-ancestor" => assert!(current.interaction_region.is_none()),
                _ => panic!("stage"),
            }
        }
        assert!(receiver.presented_revision().unwrap().is_none());
    }
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
}

#[test]
fn binding_source_geometry_and_surface_input_coverage_are_checked_before_presentation() {
    let directory = base().join("binding-golden");
    let original = CaptureArtifact::read(directory.join("binding-source-0001.json")).unwrap();
    let mut receiver = original.receiver().unwrap();
    original.apply_trusted_fixture(&mut receiver).unwrap();
    let temporary = std::env::temp_dir().join(format!("rui-binding-audit-{}", std::process::id()));
    fs::create_dir(&temporary).unwrap();
    let path = temporary.join("capture.json");
    for tamper in ["move", "omit", "duplicate", "semantic"] {
        let mut manifest = original.manifest().clone();
        match tamper {
            "move" => manifest["binding_regions"][0]["interaction_region"][0] = json!(100),
            "omit" => {
                manifest["binding_regions"]
                    .as_array_mut()
                    .unwrap()
                    .remove(0);
            }
            "duplicate" => {
                let plan = manifest["binding_regions"][0].clone();
                manifest["binding_regions"]
                    .as_array_mut()
                    .unwrap()
                    .push(plan);
            }
            "semantic" => manifest["binding_regions"][0]["semantic_id"] = json!(1),
            _ => unreachable!(),
        }
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(
            CaptureArtifact::read(&path)
                .unwrap()
                .verify_vector_audit(&receiver)
                .is_err(),
            "{tamper}"
        );
    }
    // A graph-valid surface whose native background is missing from the
    // advertised input coverage must not pass the source consistency audit.
    for offer in original.manifest()["resource_offers"].as_array().unwrap() {
        let name = format!("{}.rvp", offer["digest"].as_str().unwrap());
        fs::copy(directory.join(&name), temporary.join(name)).unwrap();
    }
    let mut envelope = pb::Envelope::decode(
        fs::read(directory.join("binding-source-0001.bin"))
            .unwrap()
            .as_slice(),
    )
    .unwrap();
    let mut snapshot = pb::Snapshot::decode(envelope.payload.as_slice()).unwrap();
    for record in &mut snapshot.records {
        if let Some(pb::record::Record::Surface(surface)) = &mut record.record {
            surface.input_region.clear();
        }
    }
    envelope.payload = snapshot.encode_to_vec();
    fs::write(path.with_extension("bin"), envelope.encode_to_vec()).unwrap();
    fs::write(&path, serde_json::to_vec(original.manifest()).unwrap()).unwrap();
    let forged = CaptureArtifact::read(&path).unwrap();
    let mut other = forged.receiver().unwrap();
    forged.apply_trusted_fixture(&mut other).unwrap();
    assert!(forged.verify_vector_audit(&other).is_err());
    assert!(other.presented_revision().unwrap().is_none());
    other.close();
    receiver.close();
    assert_eq!((other.memory_used(), receiver.memory_used()), (0, 0));
    fs::remove_dir_all(temporary).unwrap();
}
