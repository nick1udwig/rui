use rui_producer_sdk::{CaptureArtifact, CaptureError};
use rui_protocol_core::model::{
    GlyphFont, Materialization, Primitive, Record, SemanticNode, StateKey, TextRange, TextStatus,
    TriState,
};
use rui_protocol_core::receiver::{ReceiverError, ViewReceiver};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

fn golden() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gtk/golden")
}

fn semantic_id(artifact: &CaptureArtifact, identity: &str) -> u64 {
    artifact.manifest()["semantics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|semantic| semantic["identity"] == identity)
        .unwrap()["id"]
        .as_u64()
        .unwrap()
}

fn semantic(receiver: &ViewReceiver, id: u64) -> &SemanticNode {
    let Record::Semantic(semantic) = &receiver.state().unwrap().records[&id] else {
        panic!("expected native semantic object")
    };
    semantic
}

#[test]
fn native_text_collection_and_focus_clocks_survive_exact_core_replay() {
    let first = CaptureArtifact::read(golden().join("main-content-0001.json")).unwrap();
    let mut receiver = first.receiver().unwrap();
    let entry = semantic_id(&first, "entry");
    let list = semantic_id(&first, "list");
    let beta = semantic_id(&first, "row-beta");
    let gamma = semantic_id(&first, "row-gamma");
    let menu = semantic_id(&first, "menu-button");
    let surface = first.manifest()["surface_id"].as_u64().unwrap();
    let session_id = first.manifest()["session"]["id"].as_u64().unwrap();
    for revision in 1..=15 {
        let artifact =
            CaptureArtifact::read(golden().join(format!("main-content-{revision:04}.json")))
                .unwrap();
        artifact.apply_trusted_fixture(&mut receiver).unwrap();
        let text_id = semantic(&receiver, entry).text.unwrap().id;
        let Record::Text(text) = &receiver.state().unwrap().records[&text_id] else {
            panic!("missing native GtkEditable model")
        };
        assert_eq!(text.status, TextStatus::Available);
        assert!(text.editable && text.selectable);
        assert_eq!(text.version, revision);
        assert_eq!(text.text_version, if revision == 1 { 1 } else { 2 });
        if revision > 1 {
            assert_eq!(text.content.as_deref(), Some("edited source: café Ω"));
            let content = text.content.as_ref().unwrap();
            assert_eq!(content.chars().count(), 21);
            assert_eq!(content.len(), 23);
        }
        if revision >= 3 {
            assert_eq!(
                text.selection,
                Some(TextRange {
                    anchor: 19,
                    focus: 15
                })
            );
        }
        let collection = semantic(&receiver, list).collection.as_ref().unwrap();
        assert_eq!(collection.total_count, Some(3));
        assert_eq!(
            collection.version,
            match revision {
                1..=3 => 1,
                4..=13 => 2,
                14 => 3,
                15 => 4,
                _ => unreachable!(),
            }
        );
        assert_eq!(collection.items[1].id, beta);
        if revision >= 4 {
            assert_eq!(collection.selected_items, vec![collection.items[1]]);
            assert_eq!(
                semantic(&receiver, beta).states[&StateKey::Selected],
                TriState::True
            );
        } else {
            assert!(collection.selected_items.is_empty());
            assert_eq!(
                semantic(&receiver, beta).states[&StateKey::Selected],
                TriState::False
            );
        }
        if revision == 14 {
            assert_eq!(collection.materialization, Materialization::Partial);
            assert_eq!(collection.items.len(), 2);
            assert!(!receiver.state().unwrap().records.contains_key(&gamma));
            assert!(receiver.state().unwrap().retired.contains(&gamma));
        } else {
            assert_eq!(collection.materialization, Materialization::Full);
            assert_eq!(collection.items.len(), 3);
            if revision == 15 {
                assert_ne!(collection.items[2].id, gamma);
                assert_eq!(collection.items[2].id, semantic_id(&artifact, "row-gamma"));
                assert!(receiver.state().unwrap().retired.contains(&gamma));
            } else {
                assert_eq!(collection.items[2].id, gamma);
            }
        }
        let Record::Session(session) = &receiver.state().unwrap().records[&session_id] else {
            panic!("missing source focus context")
        };
        let (generation, target) = match revision {
            1..=4 => (2, Some(entry)),
            5..=6 => (3, None),
            7 => (4, Some(menu)),
            8 => (5, None),
            9..=12 => (6, Some(menu)),
            13..=15 => (7, None),
            _ => unreachable!(),
        };
        assert_eq!(session.focus_generation, generation);
        assert_eq!(session.focus.map(|reference| reference.id), target);
        assert_eq!(
            session.focus_surface.map(|reference| reference.id),
            target.map(|_| surface)
        );
        assert_eq!(
            session.canvas.width,
            if revision < 12 { 520.0 } else { 640.0 }
        );
        assert!(receiver.presented_revision().unwrap().is_none());
    }
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
}

#[test]
fn captured_content_and_collection_changes_reject_stale_model_clocks_atomically() {
    use prost::Message;
    use rui_protocol_core::wire::proto as pb;
    for (name, previous_revision, changed_revision, invalid_clock) in [
        ("text", 1, 2, 1),
        ("text-selection", 2, 3, 1),
        ("collection", 3, 4, 1),
        ("focus", 6, 7, 3),
    ] {
        let first = CaptureArtifact::read(golden().join("main-content-0001.json")).unwrap();
        let mut receiver = first.receiver().unwrap();
        for revision in 1..=previous_revision {
            CaptureArtifact::read(golden().join(format!("main-content-{revision:04}.json")))
                .unwrap()
                .apply_trusted_fixture(&mut receiver)
                .unwrap();
        }
        let before = receiver.state().unwrap().records.clone();
        let bytes =
            fs::read(golden().join(format!("main-content-{changed_revision:04}.bin"))).unwrap();
        let mut envelope = pb::Envelope::decode(bytes.as_slice()).unwrap();
        let mut snapshot = pb::Snapshot::decode(envelope.payload.as_slice()).unwrap();
        for record in &mut snapshot.records {
            match record.record.as_mut().unwrap() {
                pb::record::Record::Text(text) if name.starts_with("text") => {
                    text.text_version = invalid_clock
                }
                pb::record::Record::Semantic(semantic) if name == "collection" => {
                    if let Some(collection) = &mut semantic.collection_model {
                        collection.version = invalid_clock;
                    }
                }
                pb::record::Record::Session(session) if name == "focus" => {
                    session.focus_generation = invalid_clock
                }
                _ => {}
            }
        }
        envelope.payload = snapshot.encode_to_vec();
        let rejected = receiver.receive_envelope([99; 16], &envelope.encode_to_vec());
        assert!(
            matches!(rejected, Err(ReceiverError::Model(_))),
            "{name}: {rejected:?}"
        );
        assert_eq!(receiver.state().unwrap().revision, previous_revision);
        assert_eq!(receiver.state().unwrap().records, before);
        // Rejected input leaves neither a pending candidate nor a poisoned slot.
        let valid = CaptureArtifact::read(
            golden().join(format!("main-content-{changed_revision:04}.json")),
        )
        .unwrap();
        valid.apply_trusted_fixture(&mut receiver).unwrap();
        assert_eq!(receiver.state().unwrap().revision, changed_revision);
        receiver.close();
        assert_eq!(receiver.memory_used(), 0);
    }
}

#[test]
fn measured_and_compared_reports_bind_every_retained_exact_publication() {
    let fixture = golden().parent().unwrap().to_path_buf();
    let measurement: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.join("measurement.json")).unwrap()).unwrap();
    let comparison: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.join("pixel-comparison.json")).unwrap()).unwrap();
    assert_eq!(comparison["passed"], true);
    let measured = measurement["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["theme"] == "strict")
        .unwrap()["captures"]
        .as_array()
        .unwrap();
    let compared = comparison["captures"].as_array().unwrap();
    assert_eq!(measured.len(), 20);
    assert_eq!(compared.len(), 17);
    for capture in measured {
        let name = format!(
            "{}-{:04}.json",
            capture["surface"].as_str().unwrap(),
            capture["revision"].as_u64().unwrap()
        );
        let artifact = CaptureArtifact::read(golden().join(&name)).unwrap();
        let manifest = artifact.manifest();
        for field in [
            "binding",
            "envelope_sha256",
            "source_revision",
            "capture_boundary",
            "mapped",
        ] {
            assert_eq!(
                capture[field], manifest[field],
                "measurement disagrees at {name}:{field}"
            );
        }
        assert_eq!(capture["node_counts"], manifest["gsk_node_counts"]);
        assert_eq!(capture["gaps"], manifest["gaps"]);
        if capture["mapped"] == true {
            let pixels = compared
                .iter()
                .find(|result| result["capture"] == name)
                .unwrap();
            assert_eq!(pixels["envelope_sha256"], manifest["envelope_sha256"]);
            assert_eq!(
                pixels["local_reference_sha256"],
                manifest["local_reference"]["sha256"]
            );
            assert_eq!(pixels["pixels"]["passed"], true);
            assert_eq!(pixels["pixels"]["interior_bad_pixels"], 0);
        } else {
            assert!(!compared.iter().any(|result| result["capture"] == name));
        }
    }
}

#[test]
fn native_popup_unmap_remap_destroy_releases_pins_and_rejects_late_source_state() {
    use prost::Message;
    use rui_protocol_core::wire::proto as pb;
    let first = CaptureArtifact::read(golden().join("menu-popup-0001.json")).unwrap();
    let mut receiver = first.receiver().unwrap();
    let surface = first.manifest()["surface_id"].as_u64().unwrap();
    let root = first.manifest()["scene_root"].as_u64().unwrap();
    let semantic_root = semantic_id(&first, "menu-popup");
    first.apply_trusted_fixture(&mut receiver).unwrap();
    let digest = *receiver
        .state()
        .unwrap()
        .resource_refs()
        .iter()
        .next()
        .unwrap();
    let offer = receiver.resource_offer(digest).unwrap().clone();
    assert!(
        receiver.evict_resource(digest).is_err(),
        "mapped scene pins its outlines"
    );
    let mut previous_root = root;
    for revision in 2..=5 {
        let artifact =
            CaptureArtifact::read(golden().join(format!("menu-popup-{revision:04}.json"))).unwrap();
        artifact.apply_trusted_fixture(&mut receiver).unwrap();
        let state = receiver.state().unwrap();
        if revision == 5 {
            assert!(!state.records.contains_key(&surface));
            assert!(!state.records.contains_key(&semantic_root));
            assert!(state.retired.contains(&surface));
            assert!(state.retired.contains(&semantic_root));
        } else {
            let Record::Surface(popup) = &state.records[&surface] else {
                panic!("missing live popup identity")
            };
            assert_eq!(popup.mapped, revision == 3);
            assert_eq!(popup.semantic_root.unwrap().id, semantic_root);
            let new_root = popup.scene_root.unwrap().id;
            assert_ne!(previous_root, new_root);
            assert!(state.retired.contains(&previous_root));
            previous_root = new_root;
        }
        let Record::Session(session) =
            &state.records[&artifact.manifest()["session"]["id"].as_u64().unwrap()]
        else {
            panic!("missing popup session")
        };
        assert_eq!(
            session.focus_generation,
            if revision == 5 { 5 } else { revision + 1 }
        );
        assert_eq!(
            session.focus.map(|reference| reference.id),
            (revision == 3).then_some(semantic_root)
        );
        if revision != 3 {
            assert!(state.resource_refs().is_empty());
            assert!(matches!(
                receiver.read_resource(digest),
                Err(ReceiverError::NotNeeded)
            ));
            assert!(matches!(
                receiver.receive_resource_offer(first.context(), offer.clone()),
                Err(ReceiverError::NotNeeded)
            ));
        }
        if revision == 2 {
            receiver.evict_resource(digest).unwrap();
        }
        if revision == 3 {
            assert!(
                receiver.read_resource(digest).is_ok(),
                "remap re-acquires an evicted outline"
            );
            assert!(receiver.evict_resource(digest).is_err());
        }
    }
    let before = receiver.state().unwrap().records.clone();
    let bytes = fs::read(golden().join("menu-popup-0001.bin")).unwrap();
    let mut envelope = pb::Envelope::decode(bytes.as_slice()).unwrap();
    let mut snapshot = pb::Snapshot::decode(envelope.payload.as_slice()).unwrap();
    snapshot.revision = 6;
    for record in &mut snapshot.records {
        match record.record.as_mut().unwrap() {
            pb::record::Record::Surface(value) => value.version = 6,
            pb::record::Record::Scene(value) => value.version = 6,
            pb::record::Record::Semantic(value) => value.version = 6,
            pb::record::Record::Session(value) => value.version = 6,
            _ => unreachable!(),
        }
    }
    envelope.payload = snapshot.encode_to_vec();
    assert!(matches!(
        receiver.receive_envelope([99; 16], &envelope.encode_to_vec()),
        Err(ReceiverError::Model(_))
    ));
    assert_eq!(receiver.state().unwrap().revision, 5);
    assert_eq!(receiver.state().unwrap().records, before);
    assert!(receiver.presented_revision().unwrap().is_none());
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
}

#[test]
fn actual_submitted_gtk_frames_replay_retirement_glyph_geometry_and_resource_lifecycle() {
    let mut files = fs::read_dir(golden())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    files.sort();
    let mut receivers = BTreeMap::new();
    let mut semantics = BTreeMap::new();
    let mut glyphs = 0;
    let mut retired = 0;
    for path in &files {
        let artifact = CaptureArtifact::read(path).unwrap();
        let view = artifact.context().view_id;
        if let std::collections::btree_map::Entry::Vacant(entry) = receivers.entry(view) {
            entry.insert(artifact.receiver().unwrap());
        }
        let receiver = receivers.get_mut(&view).unwrap();
        let revision = artifact.apply_trusted_fixture(receiver).unwrap();
        artifact.verify_native_glyph_evidence(receiver).unwrap();
        assert_eq!(receiver.state().unwrap().revision, revision);
        assert!(receiver.resources_ready().unwrap());
        assert!(
            receiver.presented_revision().unwrap().is_none(),
            "no renderer ran in this replay test"
        );
        retired += receiver.state().unwrap().retired.len();
        for semantic in artifact.manifest()["semantics"].as_array().unwrap() {
            let key = (view, semantic["identity"].as_str().unwrap().to_owned());
            let id = semantic["id"].as_u64().unwrap();
            if let Some(previous) = semantics.insert(key, id) {
                if previous != id {
                    assert_eq!(semantic["identity"], "row-gamma");
                    assert!(receiver.state().unwrap().retired.contains(&previous));
                    assert!(!receiver.state().unwrap().retired.contains(&id));
                }
            }
        }
        for evidence in artifact.manifest()["source_glyph_evidence"]
            .as_array()
            .unwrap()
        {
            let id = evidence["scene_id"].as_u64().unwrap();
            let Record::Scene(scene) = &receiver.state().unwrap().records[&id] else {
                panic!("missing captured glyph scene")
            };
            let Primitive::GlyphRun {
                font: GlyphFont::Outline(resource),
                size,
                glyph_ids,
                positions,
                ..
            } = &scene.primitive
            else {
                panic!("source glyph changed representation")
            };
            assert_eq!(glyph_ids, &[0]);
            assert_eq!(*size, evidence["font_size"].as_f64().unwrap());
            let scale = evidence["pango_scale"].as_f64().unwrap();
            let x = evidence["node_offset"][0].as_f64().unwrap()
                + evidence["advance"].as_f64().unwrap()
                + evidence["pango_x_offset"].as_f64().unwrap() / scale;
            let y = evidence["node_offset"][1].as_f64().unwrap()
                + evidence["pango_y_offset"].as_f64().unwrap() / scale;
            assert_eq!(positions[0].x, x);
            assert_eq!(positions[0].y, y);
            assert_eq!(
                receiver.read_resource(*resource).unwrap().get(..4),
                Some(b"RVP1".as_slice())
            );
            glyphs += 1;
        }
    }
    assert_eq!(files.len(), 20);
    assert_eq!(
        receivers.len(),
        2,
        "main and explicitly identified popup views"
    );
    assert!(
        retired > 100,
        "conditional source content disappeared and returned with fresh IDs"
    );
    assert!(
        glyphs > 100,
        "real shaped source glyphs were verified without reshaping"
    );
    for receiver in receivers.values_mut() {
        receiver.close();
        assert_eq!(receiver.memory_used(), 0);
    }
}

#[test]
fn actual_decoded_glyph_omission_shift_and_font_size_fail_source_geometry_gate() {
    use prost::Message;
    use rui_protocol_core::wire::proto as pb;
    let artifact = CaptureArtifact::read(golden().join("main-content-0001.json")).unwrap();
    let original = fs::read(golden().join("main-content-0001.bin")).unwrap();
    for change in ["omission", "position", "size"] {
        let mut receiver = artifact.receiver().unwrap();
        artifact.apply_trusted_fixture(&mut receiver).unwrap();
        let mut envelope = pb::Envelope::decode(original.as_slice()).unwrap();
        let mut snapshot = pb::Snapshot::decode(envelope.payload.as_slice()).unwrap();
        snapshot.revision = 2;
        let mut changed = false;
        for record in &mut snapshot.records {
            match record.record.as_mut().unwrap() {
                pb::record::Record::Surface(surface) => surface.version = 2,
                pb::record::Record::Semantic(semantic) => semantic.version = 2,
                pb::record::Record::Text(text) => text.version = 2,
                pb::record::Record::Session(session) => session.version = 2,
                pb::record::Record::Scene(scene) => {
                    scene.version = 2;
                    if !changed {
                        if let Some(pb::scene_node::Primitive::GlyphRun(glyph)) =
                            &mut scene.primitive
                        {
                            match change {
                                "position" => glyph.positions[0].x += 1.0,
                                "size" => glyph.size += 1.0,
                                "omission" => {
                                    scene.primitive =
                                        Some(pb::scene_node::Primitive::Group(pb::Group {}))
                                }
                                _ => unreachable!(),
                            }
                            changed = true;
                        }
                    }
                }
                _ => panic!("unexpected foundation record"),
            }
        }
        assert!(changed);
        envelope.payload = snapshot.encode_to_vec();
        receiver
            .receive_envelope([2; 16], &envelope.encode_to_vec())
            .unwrap();
        assert_eq!(receiver.state().unwrap().revision, 2);
        assert!(matches!(
            artifact.verify_native_glyph_evidence(&receiver),
            Err(CaptureError::Invalid)
        ));
        assert!(receiver.presented_revision().unwrap().is_none());
        receiver.close();
        assert_eq!(receiver.memory_used(), 0);
    }
}

#[test]
fn producer_restart_epoch_cannot_be_replayed_into_an_existing_view() {
    use prost::Message;
    use rui_protocol_core::wire::proto::Envelope;
    let artifact = CaptureArtifact::read(golden().join("main-content-0001.json")).unwrap();
    let mut receiver = artifact.receiver().unwrap();
    artifact.apply_trusted_fixture(&mut receiver).unwrap();
    let bytes = fs::read(golden().join("main-content-0001.bin")).unwrap();
    let mut restarted = Envelope::decode(bytes.as_slice()).unwrap();
    restarted.epoch = vec![99; 16];
    assert!(receiver
        .receive_envelope([99; 16], &restarted.encode_to_vec())
        .is_err());
    assert_eq!(receiver.state().unwrap().revision, 1);
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
}

#[test]
fn resource_descriptor_length_is_checked_before_policy_grants() {
    let artifact = CaptureArtifact::read(golden().join("main-content-0001.json")).unwrap();
    let mut manifest = artifact.manifest().clone();
    manifest["resource_offers"][0]["total_bytes"] = serde_json::json!(1);
    let directory = std::env::temp_dir().join(format!("rui-sdk-length-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("mismatch.json");
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let digest = manifest["resource_offers"][0]["digest"].as_str().unwrap();
    fs::copy(
        golden().join(format!("{digest}.rvp")),
        directory.join(format!("{digest}.rvp")),
    )
    .unwrap();
    let forged = CaptureArtifact::read(&path).unwrap();
    let mut receiver = forged.receiver().unwrap();
    let before = receiver.memory_used();
    assert!(matches!(
        forged.apply_trusted_fixture(&mut receiver),
        Err(CaptureError::Invalid)
    ));
    assert_eq!(receiver.memory_used(), before);
    assert_eq!(receiver.state().unwrap().revision, 0);
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
    fs::remove_dir_all(directory).unwrap();
}
