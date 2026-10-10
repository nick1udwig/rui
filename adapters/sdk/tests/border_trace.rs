use rui_producer_sdk::CaptureArtifact;
use rui_protocol_core::model::{Primitive, Record};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

#[test]
fn submitted_square_borders_preserve_paths_clips_and_bounded_ownership() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gtk");
    let report: Value =
        serde_json::from_slice(&fs::read(base.join("border-comparison.json")).unwrap()).unwrap();
    assert_eq!(report["passed"], true);
    assert_eq!(report["negativeMissingSide"]["rejected"], true);
    let root = base.join("border-golden");
    for (name, hash) in report["files"].as_object().unwrap() {
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(root.join(name)).unwrap())),
            hash.as_str().unwrap()
        );
    }
    for i in 1..=4 {
        let artifact = CaptureArtifact::read(root.join(format!("border-{i:04}.json"))).unwrap();
        let mut receiver = artifact.receiver().unwrap();
        artifact.apply_trusted_fixture(&mut receiver).unwrap();
        let audit = artifact.verify_vector_audit(&receiver).unwrap();
        assert_eq!(audit.resource_bodies, 4);
        assert_eq!(audit.unsupported_subtrees, 0);
        assert_eq!(receiver.state().unwrap().records.values().filter(|r| matches!(r,Record::Scene(s) if matches!(s.primitive,Primitive::FillPath{..}))).count(), 4);
        receiver.close();
        assert_eq!(receiver.memory_used(), 0);
    }
}
