//! Export authored structured scenes through the production publication builder.
use prost::Message;
use rui_producer_sdk::publication_builder;
use rui_protocol_core::{
    model::{Limits, Replica},
    wire::proto as p,
};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufRead, BufReader, Write},
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene {
    id: u64,
    #[serde(default)]
    children: Vec<u64>,
    #[serde(default = "identity")]
    transform: [f64; 6],
    #[serde(default)]
    clip: Option<[f64; 4]>,
    #[serde(default)]
    segments: Option<Vec<[f64; 8]>>,
    #[serde(default)]
    rect: Option<[f64; 4]>,
    #[serde(default)]
    color: [f64; 4],
    #[serde(default = "one")]
    width: f64,
}
fn one() -> f64 {
    1.0
}
fn identity() -> [f64; 6] {
    [1., 0., 0., 1., 0., 0.]
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    nodes: Vec<Scene>,
    #[serde(default)]
    text: String,
}
fn rect(r: [f64; 4]) -> p::Rect {
    p::Rect {
        x: r[0],
        y: r[1],
        width: r[2],
        height: r[3],
    }
}
fn reference(id: u64) -> p::ObjectRef {
    p::ObjectRef {
        kind: p::ObjectKind::Scene as i32,
        id,
    }
}
fn snapshot(frame: Frame, sequence: u64) -> p::Snapshot {
    let mut records = vec![
        p::Record {
            record: Some(p::record::Record::Session(p::SessionState {
                id: 1,
                version: 1,
                canvas: Some(rect([0., 0., 640., 480.])),
                ..Default::default()
            })),
        },
        p::Record {
            record: Some(p::record::Record::Surface(p::Surface {
                id: 2,
                version: 1,
                desktop_rect: Some(rect([0., 0., 640., 480.])),
                content_size: Some(p::Size {
                    width: 640.,
                    height: 480.,
                }),
                transform: Some(p::Affine {
                    xx: 1.,
                    yy: 1.,
                    ..Default::default()
                }),
                opacity: 1.,
                mapped: true,
                scene_root: Some(reference(100)),
                semantic_root: Some(p::ObjectRef {
                    kind: p::ObjectKind::Semantic as i32,
                    id: 3,
                }),
                visual_coverage: p::VisualCoverage::Complete as i32,
                semantic_coverage: p::SemanticCoverage::SemanticPartial as i32,
                capture_consistency: p::CaptureConsistency::Coherent as i32,
                source_capture_tokens: vec![sequence.to_be_bytes().to_vec()],
                role: p::SurfaceRole::Toplevel as i32,
                ..Default::default()
            })),
        },
        p::Record {
            record: Some(p::record::Record::Semantic(p::SemanticNode {
                id: 3,
                version: 1,
                role: p::SemanticRole::Label as i32,
                label: Some(frame.text),
                sensitivity: p::Sensitivity::Public as i32,
                completeness: p::Completeness::Full as i32,
                ..Default::default()
            })),
        },
    ];
    for n in frame.nodes {
        let primitive = if let Some(data) = n.segments {
            p::scene_node::Primitive::SegmentBatch(p::SegmentBatch {
                data: data.into_iter().flatten().collect(),
                width: n.width,
                minimum_length: 0.,
            })
        } else if let Some(r) = n.rect {
            p::scene_node::Primitive::SolidRect(p::SolidRect {
                rect: Some(rect(r)),
                color: Some(p::Color {
                    red: n.color[0],
                    green: n.color[1],
                    blue: n.color[2],
                    alpha: n.color[3],
                }),
            })
        } else {
            p::scene_node::Primitive::Group(p::Group {})
        };
        records.push(p::Record {
            record: Some(p::record::Record::Scene(p::SceneNode {
                id: n.id,
                version: 1,
                ordered_children: n.children.into_iter().map(reference).collect(),
                transform: Some(p::Affine {
                    xx: n.transform[0],
                    yx: n.transform[1],
                    xy: n.transform[2],
                    yy: n.transform[3],
                    tx: n.transform[4],
                    ty: n.transform[5],
                }),
                clip: n.clip.map(rect),
                opacity: 1.,
                primitive: Some(primitive),
                ..Default::default()
            })),
        });
    }
    p::Snapshot {
        revision: 1,
        records,
        required_features: vec!["geometry.segments.v1".into()],
        ..Default::default()
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 && args.len() != 5 {
        return Err("usage: scene-trace SCENES_JSONL OUTPUT_RUI OUTPUT_JSON [START_FRAME]".into());
    }
    let start: usize = args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(0);
    let mut output = File::create(&args[2])?;
    let mut previous = p::Snapshot::default();
    let mut replica = Replica::new(Limits::default());
    let mut samples = vec![];
    for (index, line) in BufReader::new(File::open(&args[1])?).lines().enumerate() {
        let frame: Frame = serde_json::from_str(&line?)?;
        let candidate = publication_builder::build(
            &previous,
            snapshot(frame, index as u64 + 1),
            &BTreeSet::new(),
        )?;
        if index == 0 {
            replica.apply_snapshot(&candidate.snapshot.encode_to_vec(), &BTreeSet::new())?;
        } else {
            candidate.validate(&replica, &BTreeSet::new(), &Limits::default())?;
            replica.apply_transaction(&candidate.transaction.encode_to_vec(), &BTreeSet::new())?;
        }
        if index >= start {
            let is_snapshot = index == start;
            let payload = if is_snapshot {
                candidate.snapshot.encode_to_vec()
            } else {
                candidate.transaction.encode_to_vec()
            };
            let envelope = p::Envelope {
                protocol_version: 1,
                session_id: vec![1; 16],
                epoch: vec![2; 16],
                view_id: vec![4; 16],
                kind: if is_snapshot { 1 } else { 2 },
                payload,
                required_features: vec![],
            }
            .encode_to_vec();
            output.write_all(&(envelope.len() as u32).to_be_bytes())?;
            output.write_all(&envelope)?;
            samples.push(serde_json::json!({"frame":index,"bytes":envelope.len(),"mutations":candidate.transaction.mutations.len(),"snapshot":is_snapshot}));
        }
        previous = candidate.snapshot;
    }
    if samples.is_empty() {
        return Err("no output frames".into());
    }
    std::fs::write(
        &args[3],
        serde_json::to_vec_pretty(
            &serde_json::json!({"samples":samples,"reducer_equivalence_verified":true}),
        )?,
    )?;
    Ok(())
}
