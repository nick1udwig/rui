#[path = "../tests/common/mod.rs"]
mod common;
use common::*;
use rui_client_desktop::*;
use rui_protocol_core::model::*;
use rui_protocol_core::resources::*;
use std::fs::{self, File};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| "fixtures/render".into());
    fs::create_dir_all(&directory)?;
    let mut resources = Resources::default();
    let triangle = resources.add(
        ResourceKind::VectorPath,
        &[
            Command::Move(168.0, 24.0),
            Command::Line(196.0, 68.0),
            Command::Line(140.0, 68.0),
            Command::Close,
        ],
    );
    let curve = resources.add(
        ResourceKind::VectorPath,
        &[
            Command::Move(236.0, 28.0),
            Command::Cubic(246.0, 4.0, 266.0, 100.0, 286.0, 40.0),
        ],
    );
    // Authored vector glyph fixture, never a substitution for a source font.
    let glyph = resources.add(
        ResourceKind::GlyphOutline,
        &[
            Command::Move(0.0, 0.0),
            Command::Line(0.0, -1.0),
            Command::Line(0.5, -1.0),
            Command::Quad(0.85, -1.0, 0.85, -0.65),
            Command::Quad(0.85, -0.35, 0.5, -0.35),
            Command::Line(0.95, 0.0),
            Command::Line(0.65, 0.0),
            Command::Line(0.25, -0.35),
            Command::Line(0.25, 0.0),
            Command::Close,
            Command::Move(0.25, -0.55),
            Command::Line(0.45, -0.55),
            Command::Quad(0.6, -0.55, 0.6, -0.65),
            Command::Quad(0.6, -0.8, 0.45, -0.8),
            Command::Line(0.25, -0.8),
            Command::Close,
        ],
    );
    let cyan = color(0.0, 0.7, 0.8, 1.0);
    let orange = color(0.9, 0.25, 0.03, 1.0);
    let violet = color(0.45, 0.03, 0.8, 1.0);
    let mut nodes = vec![
        group(1, &[2, 3, 4, 5, 6, 7, 8, 9, 10]),
        solid(
            2,
            rect(0.0, 0.0, 320.0, 180.0),
            color(0.018, 0.025, 0.04, 1.0),
        ),
        solid(3, rect(16.0, 24.0, 46.0, 44.0), cyan),
        node(
            4,
            Primitive::RoundedRect {
                rect: rect(78.0, 24.0, 46.0, 44.0),
                radii: radius(12.0, 12.0),
                color: violet,
            },
        ),
        path(5, triangle, FillRule::Nonzero, orange),
        stroke(
            6,
            curve,
            6.0,
            StrokeCap::Round,
            StrokeJoin::Round,
            vec![9.0, 5.0],
            3.0,
        ),
        node(
            7,
            Primitive::Border {
                rect: rect(16.0, 106.0, 46.0, 44.0),
                radii: radius(10.0, 10.0),
                width: 4.0,
                color: cyan,
            },
        ),
        node(
            8,
            Primitive::LinearGradient {
                rect: rect(78.0, 106.0, 46.0, 44.0),
                start: point(78.0, 106.0),
                end: point(124.0, 150.0),
                stops: stops(violet, cyan),
                spread: GradientSpread::Pad,
            },
        ),
        node(
            9,
            Primitive::RadialGradient {
                rect: rect(140.0, 106.0, 56.0, 44.0),
                center: point(168.0, 128.0),
                inner_radius: 0.0,
                outer_radius: 28.0,
                stops: stops(orange, violet),
                spread: GradientSpread::Pad,
            },
        ),
        node(
            10,
            Primitive::GlyphRun {
                font: GlyphFont::Outline(glyph),
                size: 44.0,
                glyph_ids: vec![0],
                positions: vec![point(240.0, 150.0)],
                scalar_clusters: vec![0],
                color: cyan,
            },
        ),
    ];
    // Nested clipping/opacity in a separately positioned popup surface.
    let mut popup = group(20, &[21, 22]);
    popup.opacity = 0.85;
    popup.clip = Some(rect(0.0, 0.0, 74.0, 16.0));
    nodes.push(popup);
    nodes.push(solid(21, rect(0.0, 0.0, 74.0, 16.0), violet));
    nodes.push(solid(22, rect(35.0, 0.0, 60.0, 16.0), cyan));
    let mut s = state(nodes);
    let main = surface(100, 1, 0.0, 0.0, 320.0, 180.0);
    let mut popup = surface(101, 20, 124.0, 82.0, 74.0, 16.0);
    popup.role = SurfaceRole::Popup;
    popup.parent = Some(ObjectRef {
        kind: ObjectKind::Surface,
        id: 100,
    });
    popup.stacking_order = 1;
    s.records.insert(100, Record::Surface(main));
    s.records.insert(101, Record::Surface(popup));
    let budget = MemoryBudget::new(128 << 20);
    let renderer = Renderer::new(RenderLimits::default(), budget.clone())?;
    let frame = renderer.render_view(&s, Viewport::new(320, 180), &resources)?;
    frame.write_pam(&mut File::create(directory.join("reference.pam"))?)?;
    frame.write_ppm(
        &mut File::create(directory.join("reference.ppm"))?,
        color(1.0, 1.0, 1.0, 1.0),
    )?;
    eprintln!(
        "{}: revision {}, {} work units, {} accounted frame bytes",
        PROFILE,
        frame.revision,
        frame.work,
        budget.used()
    );
    Ok(())
}
