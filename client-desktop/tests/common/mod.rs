#![allow(dead_code)]
use rui_client_desktop::{ResourceProvider, ResourceView};
use rui_protocol_core::model::*;
use rui_protocol_core::resources::*;
use std::collections::BTreeMap;

pub fn point(x: f64, y: f64) -> Point {
    Point { x, y }
}
pub fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}
pub fn color(red: f64, green: f64, blue: f64, alpha: f64) -> Color {
    Color {
        red,
        green,
        blue,
        alpha,
    }
}
pub fn identity() -> Affine {
    Affine {
        xx: 1.0,
        yy: 1.0,
        xy: 0.0,
        yx: 0.0,
        tx: 0.0,
        ty: 0.0,
    }
}
pub fn reference(id: u64) -> ObjectRef {
    ObjectRef {
        kind: ObjectKind::Scene,
        id,
    }
}
pub fn node(id: u64, primitive: Primitive) -> SceneNode {
    SceneNode {
        id,
        version: 1,
        children: vec![],
        transform: identity(),
        clip: None,
        opacity: 1.0,
        primitive,
        resources: Default::default(),
    }
}
pub fn state(nodes: impl IntoIterator<Item = SceneNode>) -> State {
    State {
        revision: 1,
        records: nodes
            .into_iter()
            .map(|n| (n.id, Record::Scene(n)))
            .collect(),
        retired: Default::default(),
    }
}
pub fn solid(id: u64, r: Rect, c: Color) -> SceneNode {
    node(id, Primitive::SolidRect { rect: r, color: c })
}
pub fn group(id: u64, children: &[u64]) -> SceneNode {
    let mut n = node(id, Primitive::Group);
    n.children = children.iter().copied().map(reference).collect();
    n
}
pub fn radius(x: f64, y: f64) -> CornerRadii {
    let r = Size {
        width: x,
        height: y,
    };
    CornerRadii {
        top_left: r,
        top_right: r,
        bottom_right: r,
        bottom_left: r,
    }
}
pub fn stops(a: Color, b: Color) -> Vec<GradientStop> {
    vec![
        GradientStop {
            offset: 0.0,
            color: a,
        },
        GradientStop {
            offset: 1.0,
            color: b,
        },
    ]
}
pub fn surface(id: u64, root: u64, x: f64, y: f64, width: f64, height: f64) -> Surface {
    Surface {
        id,
        version: 1,
        role: SurfaceRole::Toplevel,
        parent: None,
        desktop_rect: rect(x, y, width, height),
        content_size: Size { width, height },
        transform: identity(),
        clip: None,
        opacity: 1.0,
        stacking_order: 0,
        mapped: true,
        focused: false,
        modal_for: None,
        scene_root: Some(reference(root)),
        semantic_root: None,
        input_region: vec![],
        routing_token: vec![],
        visual_coverage: VisualCoverage::Complete,
        semantic_coverage: SemanticCoverage::Opaque,
        capture_consistency: CaptureConsistency::Coherent,
        source_capture_tokens: vec![vec![1]],
        unsupported_features: vec![],
    }
}
pub enum Command {
    Move(f64, f64),
    Line(f64, f64),
    Quad(f64, f64, f64, f64),
    Cubic(f64, f64, f64, f64, f64, f64),
    Close,
}
pub fn vector(commands: &[Command]) -> Vec<u8> {
    let mut b = b"RVP1".to_vec();
    b.extend_from_slice(&(commands.len() as u32).to_be_bytes());
    for c in commands {
        let (op, v) = match *c {
            Command::Move(x, y) => (1, vec![x, y]),
            Command::Line(x, y) => (2, vec![x, y]),
            Command::Quad(x, y, u, v) => (3, vec![x, y, u, v]),
            Command::Cubic(x, y, u, v, s, t) => (4, vec![x, y, u, v, s, t]),
            Command::Close => (5, vec![]),
        };
        b.push(op);
        for n in v {
            b.extend_from_slice(&n.to_be_bytes());
        }
    }
    b
}
#[derive(Default)]
pub struct Resources(pub BTreeMap<[u8; 32], (ResourceOffer, Vec<u8>)>);
impl Resources {
    pub fn add(&mut self, kind: ResourceKind, commands: &[Command]) -> [u8; 32] {
        let bytes = vector(commands);
        self.add_bytes(kind, bytes)
    }
    pub fn add_bytes(&mut self, kind: ResourceKind, bytes: Vec<u8>) -> [u8; 32] {
        let provenance = match kind {
            ResourceKind::VectorPath => ResourceProvenance::AuthoredVector,
            ResourceKind::GlyphOutline => ResourceProvenance::FontOutline,
        };
        let digest = resource_digest(kind, 1, provenance, &bytes);
        let offer = ResourceOffer {
            kind,
            version: 1,
            total_bytes: bytes.len() as u64,
            provenance,
            digest,
        };
        self.0.insert(digest, (offer, bytes));
        digest
    }
}
impl ResourceProvider for Resources {
    fn get(&self, id: &[u8; 32]) -> Option<ResourceView<'_>> {
        self.0
            .get(id)
            .map(|(offer, bytes)| ResourceView { offer, bytes })
    }
}
pub fn box_path(x: f64, y: f64, w: f64, h: f64) -> Vec<Command> {
    vec![
        Command::Move(x, y),
        Command::Line(x + w, y),
        Command::Line(x + w, y + h),
        Command::Line(x, y + h),
        Command::Close,
    ]
}
pub fn path(id: u64, resource: [u8; 32], fill_rule: FillRule, c: Color) -> SceneNode {
    node(
        id,
        Primitive::FillPath {
            resource,
            color: c,
            fill_rule,
        },
    )
}
pub fn stroke(
    id: u64,
    resource: [u8; 32],
    width: f64,
    cap: StrokeCap,
    join: StrokeJoin,
    dashes: Vec<f64>,
    offset: f64,
) -> SceneNode {
    node(
        id,
        Primitive::StrokePath {
            resource,
            color: color(1.0, 0.0, 0.0, 1.0),
            width,
            cap,
            join,
            miter_limit: if join == StrokeJoin::Miter { 4.0 } else { 1.0 },
            dash_lengths: dashes,
            dash_offset: offset,
        },
    )
}
