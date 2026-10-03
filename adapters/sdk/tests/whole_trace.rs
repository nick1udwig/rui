//! Actual native four-surface transaction/lifecycle and decoded audit evidence.
use rui_producer_sdk::CaptureArtifact;
use rui_protocol_core::model::{CaptureConsistency, Primitive, Record};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::PathBuf};

fn base() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gtk")
}
fn semantic(manifest: &Value, key: &str) -> u64 {
    manifest["semantics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["identity"] == key)
        .unwrap()["id"]
        .as_u64()
        .unwrap()
}
fn surface(manifest: &Value, key: &str) -> u64 {
    manifest["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["surface_key"] == key)
        .unwrap()["surface_id"]
        .as_u64()
        .unwrap()
}

#[test]
fn actual_whole_view_barrier_retirement_focus_modality_closure_and_pixels() {
    let report: Value =
        serde_json::from_slice(&fs::read(base().join("whole-interaction.json")).unwrap()).unwrap();
    let root = base().join("whole-golden");
    let files: BTreeSet<_> = fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let expected: BTreeSet<_> = report["retained_trace_sha256"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(files, expected);
    assert_eq!(files.len(), 47);
    for (name, hash) in report["retained_trace_sha256"].as_object().unwrap() {
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(root.join(name)).unwrap())),
            hash.as_str().unwrap()
        );
        assert!(matches!(
            root.join(name).extension().unwrap().to_str().unwrap(),
            "json" | "bin" | "rvp"
        ));
    }
    assert_eq!(report["capture_budget"]["after_close"], 0);
    assert!(
        report["capture_budget"]["peak"].as_u64().unwrap()
            <= report["capture_budget"]["limit"].as_u64().unwrap()
    );
    assert!(report["pixels"]["passed"].as_bool().unwrap());
    for pixel in report["pixels"]["captures"].as_array().unwrap() {
        assert_eq!(pixel["pixels"]["interior_bad_pixels"], 0);
        assert!(pixel["geometry"]["glyphs_checked"].as_u64().unwrap() >= 100);
        assert!(
            pixel["geometry"]["maximum_source_pixel_bounds_error"]
                .as_f64()
                .unwrap()
                < 1.0 / 256.0
        );
    }
    let interrupted = report["interrupted_nonce"].as_u64().unwrap();
    let events = report["barrier_events"].as_array().unwrap();
    assert!(events
        .iter()
        .any(|e| e["nonce"] == interrupted && e["event"] == "fresh-capture"));
    assert!(events
        .iter()
        .any(|e| e["nonce"] == interrupted && e["event"] == "abort"));
    assert!(!events
        .iter()
        .any(|e| e["nonce"] == interrupted && e["event"] == "commit"));
    let stale_tokens: BTreeSet<_> = events
        .iter()
        .filter(|e| e["nonce"] == interrupted && e["event"] == "fresh-capture")
        .map(|e| e["token"].as_str().unwrap())
        .collect();
    let first = CaptureArtifact::read(root.join("whole-0001.json")).unwrap();
    let main = surface(first.manifest(), "main-content");
    let menu = surface(first.manifest(), "menu-popup");
    let tooltip = surface(first.manifest(), "tooltip");
    let modal = surface(first.manifest(), "modal");
    let tooltip_semantic = semantic(first.manifest(), "tooltip-root");
    let menu_semantic = semantic(first.manifest(), "menu-popup");
    let heading = semantic(first.manifest(), "heading");
    let entry = semantic(first.manifest(), "entry");
    let modal_entry = semantic(first.manifest(), "modal-entry");
    let session_id = first.manifest()["session"]["id"].as_u64().unwrap();
    let mut receiver = first.receiver().unwrap();
    let mut entry_clocks = None;
    for revision in 1..=5 {
        let artifact =
            CaptureArtifact::read(root.join(format!("whole-{revision:04}.json"))).unwrap();
        assert_eq!(artifact.context(), first.context());
        artifact.apply_trusted_fixture(&mut receiver).unwrap();
        artifact.verify_vector_audit(&receiver).unwrap();
        let m = artifact.manifest();
        assert_ne!(m["capture_barrier_nonce"], interrupted);
        for s in m["surfaces"].as_array().unwrap() {
            assert!(!stale_tokens.contains(s["capture_token"].as_str().unwrap()));
            assert_eq!(s["capture_barrier_nonce"], m["capture_barrier_nonce"]);
        }
        let state = receiver.state().unwrap();
        assert_eq!(state.revision, revision);
        let surfaces = state
            .records
            .values()
            .filter_map(|r| {
                if let Record::Surface(s) = r {
                    Some(s)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(
            surfaces.len(),
            match revision {
                1 => 4,
                2..=4 => 3,
                _ => 2,
            }
        );
        assert_eq!(
            surfaces.iter().filter(|s| s.mapped).count(),
            match revision {
                1 => 4,
                2 => 3,
                4 => 2,
                _ => 1,
            }
        );
        for s in &surfaces {
            assert_eq!(s.capture_consistency, CaptureConsistency::Coherent);
            if s.id != main {
                assert_eq!(s.parent.unwrap().id, main);
            }
            if !s.mapped {
                let Record::Scene(scene) = &state.records[&s.scene_root.unwrap().id] else {
                    panic!("unmapped root")
                };
                assert!(matches!(scene.primitive, Primitive::Group));
                assert!(scene.children.is_empty());
            }
        }
        let Record::Surface(modal_surface) = &state.records[&modal] else {
            panic!("modal")
        };
        assert_eq!(
            modal_surface.modal_for.map(|r| r.id),
            if revision <= 2 { Some(main) } else { None }
        );
        let Record::Session(session) = &state.records[&session_id] else {
            panic!("session")
        };
        if revision <= 2 {
            assert_eq!(session.focus.unwrap().id, modal_entry);
            assert_eq!(session.focus_surface.unwrap().id, modal);
        } else if revision == 3 {
            assert_eq!(session.focus.unwrap().id, entry);
            assert_eq!(session.focus_surface.unwrap().id, main);
        }
        let Record::Semantic(entry_node) = &state.records[&entry] else {
            panic!("entry")
        };
        let Record::Text(text) = &state.records[&entry_node.text.unwrap().id] else {
            panic!("text")
        };
        if revision == 1 {
            entry_clocks = Some((text.version, text.text_version));
        }
        if revision == 2 {
            assert_eq!(entry_clocks, Some((text.version, text.text_version)));
        }
        if revision >= 2 {
            assert!(!state.records.contains_key(&tooltip));
            assert!(!state.records.contains_key(&tooltip_semantic));
            assert!(state.retired.contains(&tooltip));
            let Record::Semantic(node) = &state.records[&heading] else {
                panic!("heading")
            };
            assert_eq!(
                node.label.as_deref(),
                Some("Barrier semantic mutation retained")
            );
        }
        if revision == 3 || revision == 4 {
            assert_eq!(surface(m, "menu-popup"), menu);
            assert_eq!(semantic(m, "menu-popup"), menu_semantic);
        }
        if revision == 5 {
            assert!(!state.records.contains_key(&menu));
            assert!(state.retired.contains(&menu));
            assert!(state.retired.contains(&menu_semantic));
        }
        assert!(receiver.presented_revision().unwrap().is_none());
    }
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
}

#[test]
fn forged_whole_barrier_membership_nonce_geometry_and_visual_clock_fail_audit() {
    let root = base().join("whole-golden");
    let artifact = CaptureArtifact::read(root.join("whole-0001.json")).unwrap();
    let mut receiver = artifact.receiver().unwrap();
    artifact.apply_trusted_fixture(&mut receiver).unwrap();
    let temporary = std::env::temp_dir().join(format!("rui-whole-audit-{}", std::process::id()));
    fs::create_dir_all(&temporary).unwrap();
    for name in ["nonce", "surface", "origin", "focus", "modal", "policy"] {
        let mut m = artifact.manifest().clone();
        match name {
            "nonce" => m["surfaces"][0]["capture_barrier_nonce"] = 99.into(),
            "surface" => {
                m["surfaces"].as_array_mut().unwrap().pop();
            }
            "origin" => m["surfaces"][0]["origin"][0] = 30.into(),
            "focus" => m["source_boundary_facts"]["focus"][0] = "main-content".into(),
            "modal" => {
                m["source_boundary_facts"]["surfaces"]["modal"]["native_modal"] = false.into()
            }
            _ => m["source_boundary_facts"]["visual_policy"]["cursor_blink"] = true.into(),
        }
        let path = temporary.join(format!("{name}.json"));
        fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
        fs::copy(root.join("whole-0001.bin"), path.with_extension("bin")).unwrap();
        assert!(CaptureArtifact::read(&path)
            .unwrap()
            .verify_vector_audit(&receiver)
            .is_err());
    }
    fs::remove_dir_all(temporary).unwrap();
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
}

#[test]
fn decoded_explicit_resource_dependency_absent_from_source_scene_fails_audit() {
    use prost::Message;
    use rui_protocol_core::wire::proto as pb;
    let root = base().join("whole-golden");
    let artifact = CaptureArtifact::read(root.join("whole-0001.json")).unwrap();
    let temporary =
        std::env::temp_dir().join(format!("rui-whole-resource-audit-{}", std::process::id()));
    fs::create_dir_all(&temporary).unwrap();
    for entry in fs::read_dir(&root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "rvp") {
            fs::copy(&path, temporary.join(path.file_name().unwrap())).unwrap();
        }
    }
    let mut envelope =
        pb::Envelope::decode(fs::read(root.join("whole-0001.bin")).unwrap().as_slice()).unwrap();
    let mut snapshot = pb::Snapshot::decode(envelope.payload.as_slice()).unwrap();
    let extra = snapshot.required_resources[0].clone();
    let scene = snapshot
        .records
        .iter_mut()
        .find_map(|record| match record.record.as_mut() {
            Some(pb::record::Record::Scene(scene)) => Some(scene),
            _ => None,
        })
        .unwrap();
    scene.resource_refs.push(extra);
    envelope.payload = snapshot.encode_to_vec();
    let bytes = envelope.encode_to_vec();
    let mut manifest = artifact.manifest().clone();
    manifest["envelope_sha256"] = format!("{:x}", Sha256::digest(&bytes)).into();
    manifest["protocol_handoff"]["envelope_bytes"] = bytes.len().into();
    let path = temporary.join("altered.json");
    fs::write(path.with_extension("bin"), bytes).unwrap();
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let altered = CaptureArtifact::read(&path).unwrap();
    let mut receiver = altered.receiver().unwrap();
    altered.apply_trusted_fixture(&mut receiver).unwrap();
    assert!(altered.verify_vector_audit(&receiver).is_err());
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
    fs::remove_dir_all(temporary).unwrap();
}
