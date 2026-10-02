use rui_producer_sdk::{CaptureArtifact, CaptureError};
use rui_protocol_core::model::{GlyphFont, Primitive, Record};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

fn golden() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gtk/golden")
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
                assert_eq!(previous, id);
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
    assert_eq!(files.len(), 10);
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
