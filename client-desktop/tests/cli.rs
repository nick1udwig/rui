use prost::Message;
use rui_protocol_core::wire::proto as p;
use std::fs;
use std::process::Command;

#[test]
fn exact_snapshot_cli_renders_local_pam_and_rejects_missing_primitive() {
    let directory = std::env::temp_dir().join(format!(
        "rui-render-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&directory).unwrap();
    let transform = p::Affine {
        xx: 1.0,
        yy: 1.0,
        ..Default::default()
    };
    let rect = p::Rect {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };
    let snapshot = p::Snapshot {
        revision: 1,
        required_features: vec!["mirror2d.core.v1".into()],
        required_resources: vec![],
        records: vec![
            p::Record {
                record: Some(p::record::Record::Scene(p::SceneNode {
                    id: 1,
                    version: 1,
                    transform: Some(transform),
                    opacity: 1.0,
                    primitive: Some(p::scene_node::Primitive::SolidRect(p::SolidRect {
                        rect: Some(rect),
                        color: Some(p::Color {
                            red: 1.0,
                            alpha: 1.0,
                            ..Default::default()
                        }),
                    })),
                    ..Default::default()
                })),
            },
            p::Record {
                record: Some(p::record::Record::Surface(p::Surface {
                    id: 2,
                    version: 1,
                    role: 1,
                    desktop_rect: Some(rect),
                    content_size: Some(p::Size {
                        width: 1.0,
                        height: 1.0,
                    }),
                    transform: Some(transform),
                    opacity: 1.0,
                    mapped: true,
                    scene_root: Some(p::ObjectRef { kind: 2, id: 1 }),
                    visual_coverage: 1,
                    semantic_coverage: 3,
                    capture_consistency: 1,
                    source_capture_tokens: vec![vec![1]],
                    ..Default::default()
                })),
            },
        ],
    };
    let input = directory.join("snapshot.pb");
    let descriptors = directory.join("resources.tsv");
    let output = directory.join("output.pam");
    fs::write(&input, snapshot.encode_to_vec()).unwrap();
    fs::write(&descriptors, b"").unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_render-snapshot"))
            .arg(&input)
            .arg(&descriptors)
            .arg(&directory)
            .arg(&output)
            .args(["1", "1"])
            .output()
            .unwrap()
    };
    let result = run();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = fs::read(&output).unwrap();
    assert!(bytes.starts_with(b"P7\nWIDTH 1\nHEIGHT 1\n"));
    assert!(bytes.ends_with(&[255, 0, 0, 255]));
    let mut invalid = snapshot;
    let Some(p::record::Record::Scene(scene)) = &mut invalid.records[0].record else {
        panic!()
    };
    scene.primitive = None;
    fs::write(&input, invalid.encode_to_vec()).unwrap();
    let result = run();
    assert!(!result.status.success());
    assert_eq!(fs::read(&output).unwrap(), bytes);
    fs::remove_dir_all(directory).unwrap();
}
