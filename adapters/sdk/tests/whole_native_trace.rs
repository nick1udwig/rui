//! One admitted native callback owns one fresh cooperating whole-view capture.
use prost::Message;
use rui_producer_sdk::CaptureArtifact;
use rui_protocol_core::model::{CaptureConsistency, Record, SemanticNode, TextRange};
use rui_protocol_core::receiver::{ReceiveOutcome, ReceiverConfig, ViewReceiver};
use rui_protocol_core::resources::{ResourceContext, ResourceScope};
use rui_protocol_core::session::ReceiverLimits;
use rui_protocol_core::wire::proto as p;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::PathBuf};

fn identity(manifest: &Value, key: &str) -> u64 {
    manifest["semantics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["identity"] == key)
        .unwrap()["id"]
        .as_u64()
        .unwrap()
}
fn semantic(receiver: &ViewReceiver, id: u64) -> &SemanticNode {
    let Record::Semantic(node) = &receiver.state().unwrap().records[&id] else {
        panic!("semantic")
    };
    node
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
fn admitted_native_whole_commands_bind_effects_modality_lifecycle_and_fresh_union() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gtk");
    let report: Value =
        serde_json::from_slice(&fs::read(base.join("whole-native-interaction.json")).unwrap())
            .unwrap();
    let root = base.join("whole-native-golden");
    let directory = root.join("whole-native");
    let evidence = &report["whole_native"];
    let actual: BTreeSet<_> = fs::read_dir(&directory)
        .unwrap()
        .map(|e| format!("whole-native/{}", e.unwrap().file_name().to_string_lossy()))
        .collect();
    let hashes = report["retained_trace_sha256"].as_object().unwrap();
    assert_eq!(actual, hashes.keys().cloned().collect());
    assert_eq!(actual.len(), 63);
    for (name, hash) in hashes {
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(root.join(name)).unwrap())),
            hash.as_str().unwrap()
        );
        assert!(matches!(
            root.join(name).extension().unwrap().to_str().unwrap(),
            "json" | "bin" | "rvp"
        ));
    }
    assert_eq!(evidence["native_dispatches"], 7);
    assert!(evidence["native_callbacks"].as_u64().unwrap() >= 7);
    assert!(evidence["retained_native_artifact_bytes"].as_u64().unwrap() < 128 << 20);
    let commands = evidence["commands"].as_array().unwrap();
    assert_eq!(commands.len(), 13);
    assert_eq!(
        commands
            .iter()
            .filter(|c| c["status"] == "Rejected")
            .count(),
        4
    );
    for command in commands.iter().filter(|c| c["status"] == "Rejected") {
        assert_eq!(command["rejection"], "MissingTarget");
        assert!(command["source_commit"].is_null());
    }
    for command in commands
        .iter()
        .filter(|c| c["retained_result_equal"] == true)
    {
        assert_eq!(command["received_native_counters_unchanged"], true);
        let original = commands
            .iter()
            .find(|c| c["command_seq"] == command["command_seq"] && c["source_commit"].is_string())
            .unwrap();
        assert_eq!(
            original["exact_semantic_wire_hex"],
            command["exact_semantic_wire_hex"]
        );
    }

    let first = CaptureArtifact::read(directory.join("snapshot-01-000001.json")).unwrap();
    let mut private = first.receiver().unwrap();
    let main = surface(first.manifest(), "main-content");
    let menu = surface(first.manifest(), "menu-popup");
    let modal = surface(first.manifest(), "modal");
    let tooltip = surface(first.manifest(), "tooltip");
    let tooltip_root = identity(first.manifest(), "tooltip-root");
    let entry = identity(first.manifest(), "entry");
    let modal_entry = identity(first.manifest(), "modal-entry");
    let button = identity(first.manifest(), "button");
    let menu_item = identity(first.manifest(), "menu-item");
    let menu_button = identity(first.manifest(), "menu-button");
    let menu_root = identity(first.manifest(), "menu-popup");
    let heading = identity(first.manifest(), "heading");
    let session_id = first.manifest()["session"]["id"].as_u64().unwrap();
    let initial_bytes = fs::read(directory.join("public-000001.bin")).unwrap();
    let envelope = p::Envelope::decode(initial_bytes.as_slice()).unwrap();
    let context = ResourceContext {
        scope: ResourceScope {
            session_id: envelope.session_id.try_into().unwrap(),
            epoch: envelope.epoch.try_into().unwrap(),
            principal_id: [4; 16],
        },
        view_id: envelope.view_id.try_into().unwrap(),
    };
    let mut public = ViewReceiver::new(
        context,
        ReceiverLimits::default(),
        ReceiverConfig::default(),
    )
    .unwrap();
    let interrupted = evidence["final_whole_capture"]["interrupted_nonce"]
        .as_u64()
        .unwrap();
    let events = evidence["final_whole_capture"]["barrier_events"]
        .as_array()
        .unwrap();
    assert!(events
        .iter()
        .any(|e| e["nonce"] == interrupted && e["event"] == "fresh-capture"));
    assert!(events
        .iter()
        .any(|e| e["nonce"] == interrupted && e["event"] == "abort"));
    assert!(!events
        .iter()
        .any(|e| e["nonce"] == interrupted && e["event"] == "commit"));
    let stale: BTreeSet<_> = events
        .iter()
        .filter(|e| e["nonce"] == interrupted && e["event"] == "fresh-capture")
        .map(|e| e["token"].as_str().unwrap())
        .collect();
    let mut button_clock = None;
    for revision in 1..=8 {
        let artifact =
            CaptureArtifact::read(directory.join(format!("snapshot-01-{revision:06}.json")))
                .unwrap();
        let manifest = artifact.manifest();
        assert_eq!(artifact.context(), first.context());
        assert_eq!(manifest["source_generation"], 1);
        artifact.apply_trusted_fixture(&mut private).unwrap();
        artifact.verify_vector_audit(&private).unwrap();
        artifact.verify_native_glyph_evidence(&private).unwrap();
        assert!(private.resources_ready().unwrap());
        assert!(private.presented_revision().unwrap().is_none());
        assert_eq!(
            manifest["fallback_probe"]["forbidden_export_calls"],
            serde_json::json!({})
        );
        assert!(
            manifest["capture_budget"]["peak"].as_u64().unwrap()
                <= manifest["capture_budget"]["limit"].as_u64().unwrap()
        );
        assert_ne!(manifest["capture_barrier_nonce"], interrupted);
        for s in manifest["surfaces"].as_array().unwrap() {
            assert_eq!(
                s["capture_barrier_nonce"],
                manifest["capture_barrier_nonce"]
            );
            assert!(!stale.contains(s["capture_token"].as_str().unwrap()));
        }
        let state = private.state().unwrap();
        let Record::Session(session) = &state.records[&session_id] else {
            panic!("session")
        };
        let Record::Surface(modal_surface) = &state.records[&modal] else {
            panic!("modal")
        };
        assert_eq!(modal_surface.mapped, revision <= 3);
        assert_eq!(
            modal_surface.modal_for.map(|r| r.id),
            if revision <= 3 { Some(main) } else { None }
        );
        assert_eq!(
            session.focus_surface.unwrap().id,
            if revision <= 3 {
                modal
            } else if revision == 6 {
                menu
            } else {
                main
            }
        );
        assert_eq!(
            session.focus.unwrap().id,
            if revision <= 3 {
                modal_entry
            } else if revision == 4 {
                entry
            } else if revision == 6 {
                menu_root
            } else {
                menu_button
            }
        );
        for record in state.records.values() {
            if let Record::Surface(surface) = record {
                assert_eq!(surface.capture_consistency, CaptureConsistency::Coherent);
            }
        }
        let Record::Surface(menu_surface) = &state.records[&menu] else {
            panic!("menu")
        };
        assert_eq!(menu_surface.mapped, matches!(revision, 1..=4 | 6));
        assert_eq!(identity(manifest, "menu-item"), menu_item);
        let clock = semantic(&private, button).version;
        assert_eq!(
            *button_clock.get_or_insert(clock),
            clock,
            "unrelated scene/topology updates preserve action clock"
        );
        if revision >= 3 {
            let Record::Text(text) =
                &state.records[&semantic(&private, modal_entry).text.unwrap().id]
            else {
                panic!("text")
            };
            assert_eq!(text.content.as_deref(), Some("Modal 𝄞 é שלום"));
            assert_eq!(
                text.selection,
                Some(TextRange {
                    anchor: 15,
                    focus: 15
                })
            );
        }
        if revision == 7 {
            assert_eq!(
                semantic(&private, heading).label.as_deref(),
                Some("Native menu action applied")
            );
        }
        if revision == 8 {
            assert!(!state.records.contains_key(&tooltip));
            assert!(!state.records.contains_key(&tooltip_root));
            assert!(state.retired.contains(&tooltip) && state.retired.contains(&tooltip_root));
            assert_eq!(
                semantic(&private, heading).label.as_deref(),
                Some("Barrier semantic mutation retained")
            );
        }
        let bytes = fs::read(directory.join(format!("public-{revision:06}.bin"))).unwrap();
        assert!(
            matches!(public.receive_envelope((revision as u128).to_be_bytes(), &bytes).unwrap(), ReceiveOutcome::Applied{revision:actual} if actual == revision)
        );
        let public_state = public.state().unwrap();
        assert!(public_state.resource_refs().is_empty());
        assert!(public_state
            .records
            .values()
            .all(|r| !matches!(r, Record::Scene(_) | Record::Binding(_))));
        assert!(public.presented_revision().unwrap().is_none());
        if revision > 1 {
            let command = commands
                .iter()
                .find(|c| c["published_revision"] == revision)
                .unwrap();
            assert_eq!(command["source_commit"], manifest["capture_token"]);
            assert_eq!(
                command["decoded_client_state"],
                serde_json::to_value(public_state).unwrap()
            );
            assert_eq!(
                manifest["command_cause"]["authority_gate"],
                "core-ledger-admitted"
            );
            assert_eq!(manifest["command_cause"]["source_generation"], 1);
            assert!(
                manifest["command_cause"]["private_request_id"]
                    .as_u64()
                    .unwrap()
                    > 0
            );
        }
    }
    let pixels = &report["pixels"];
    assert_eq!(pixels["passed"], true);
    assert_eq!(pixels["captures"].as_array().unwrap().len(), 8);
    for pixel in pixels["captures"].as_array().unwrap() {
        let label = pixel["capture"].as_str().unwrap();
        let revision: u64 = label[6..10].parse().unwrap();
        let artifact =
            CaptureArtifact::read(directory.join(format!("snapshot-01-{revision:06}.json")))
                .unwrap();
        assert_eq!(
            pixel["envelope_sha256"],
            artifact.manifest()["envelope_sha256"]
        );
        assert_eq!(
            pixel["local_reference_sha256"],
            artifact.manifest()["local_reference"]["sha256"]
        );
        assert_eq!(pixel["pixels"]["interior_bad_pixels"], 0);
        assert_eq!(
            pixel["geometry"]["glyphs_checked"].as_u64().unwrap() as usize,
            artifact.manifest()["source_glyph_evidence"]
                .as_array()
                .unwrap()
                .len()
        );
    }
    private.close();
    public.close();
    assert_eq!(private.memory_used(), 0);
    assert_eq!(public.memory_used(), 0);
}
