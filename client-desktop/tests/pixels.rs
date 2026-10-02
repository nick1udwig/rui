mod common;
use common::*;
use rui_client_desktop::*;
use rui_protocol_core::model::*;
use rui_protocol_core::resources::*;

fn renderer() -> Renderer {
    Renderer::new(RenderLimits::default(), MemoryBudget::new(128 << 20)).unwrap()
}
fn render(nodes: Vec<SceneNode>, w: u32, h: u32, resources: &impl ResourceProvider) -> Frame {
    renderer()
        .render_scene(&state(nodes), reference(1), Viewport::new(w, h), resources)
        .unwrap()
}
fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-10, "{a} != {b}");
}
fn pixel(f: &Frame, x: u32, y: u32, r: f64, g: f64, b: f64, a: f64) {
    let p = f.pixel(x, y).unwrap();
    close(p.red, r);
    close(p.green, g);
    close(p.blue, b);
    close(p.alpha, a);
}
fn red() -> Color {
    color(1.0, 0.0, 0.0, 1.0)
}
fn blue() -> Color {
    color(0.0, 0.0, 1.0, 1.0)
}

#[test]
fn half_open_rectangle_coverage_uses_four_fixed_samples() {
    let f = render(vec![solid(1, rect(0.25, 0.25, 0.5, 0.5), red())], 2, 2, &());
    pixel(&f, 0, 0, 0.25, 0.0, 0.0, 0.25);
    pixel(&f, 1, 0, 0.0, 0.0, 0.0, 0.0);
    assert_eq!(f.pixel(2, 0), None);
    assert_eq!(f.revision, 1);
}
#[test]
fn source_over_and_output_transfer_are_linear_srgb() {
    let f = render(
        vec![
            group(1, &[2, 3]),
            solid(2, rect(0.0, 0.0, 1.0, 1.0), blue()),
            solid(3, rect(0.0, 0.0, 1.0, 1.0), color(0.5, 0.0, 0.0, 0.5)),
        ],
        1,
        1,
        &(),
    );
    pixel(&f, 0, 0, 0.5, 0.0, 0.5, 1.0);
    assert_eq!(f.pixel(0, 0).unwrap().rgba8(), [188, 0, 188, 255]);
    let mut bytes = vec![];
    f.write_pam(&mut bytes).unwrap();
    assert!(bytes.ends_with(&[188, 0, 188, 255]));
}
#[test]
fn opacity_applies_after_isolated_group_composition() {
    let mut g = group(1, &[2, 3]);
    g.opacity = 0.5;
    let f = render(
        vec![
            g,
            solid(2, rect(0.0, 0.0, 2.0, 1.0), red()),
            solid(3, rect(0.0, 0.0, 1.0, 1.0), blue()),
        ],
        2,
        1,
        &(),
    );
    pixel(&f, 0, 0, 0.0, 0.0, 0.5, 0.5);
    pixel(&f, 1, 0, 0.5, 0.0, 0.0, 0.5);
}
#[test]
fn ancestor_clip_transform_and_observer_scale_preserve_state() {
    let mut g = group(1, &[2]);
    g.transform.tx = 1.0;
    g.clip = Some(rect(0.0, 0.0, 1.0, 1.0));
    let mut n = solid(2, rect(0.0, 0.0, 2.0, 2.0), red());
    n.transform.xx = 2.0;
    let s = state([g, n]);
    let before = s.clone();
    let r = renderer();
    let f = r
        .render_scene(&s, reference(1), Viewport::new(4, 2), &())
        .unwrap();
    pixel(&f, 0, 0, 0.0, 0.0, 0.0, 0.0);
    pixel(&f, 1, 0, 1.0, 0.0, 0.0, 1.0);
    pixel(&f, 2, 0, 0.0, 0.0, 0.0, 0.0);
    let mut v = Viewport::new(4, 2);
    v.scale = 2.0;
    v.origin = point(1.0, 0.0);
    let f = r.render_scene(&s, reference(1), v, &()).unwrap();
    pixel(&f, 1, 1, 1.0, 0.0, 0.0, 1.0);
    pixel(&f, 2, 1, 0.0, 0.0, 0.0, 0.0);
    assert_eq!(s, before);
}
#[test]
fn rotated_and_singular_geometry() {
    let mut n = solid(1, rect(0.0, 0.0, 2.0, 1.0), red());
    n.transform = Affine {
        xx: 0.0,
        xy: -1.0,
        yx: 1.0,
        yy: 0.0,
        tx: 1.0,
        ty: 0.0,
    };
    let f = render(vec![n.clone()], 2, 2, &());
    pixel(&f, 0, 1, 1.0, 0.0, 0.0, 1.0);
    pixel(&f, 1, 0, 0.0, 0.0, 0.0, 0.0);
    n.transform = identity();
    n.transform.xx = 0.0;
    let f = render(vec![n], 2, 2, &());
    assert!(f.pixels().iter().all(|p| *p == LinearPixel::default()));
}
#[test]
fn surfaces_use_absolute_placement_stable_stacking_and_content_clip() {
    let mut s = state([
        solid(1, rect(0.0, 0.0, 8.0, 8.0), red()),
        solid(2, rect(0.0, 0.0, 8.0, 8.0), blue()),
    ]);
    let mut a = surface(10, 1, 1.0, 0.0, 2.0, 1.0);
    a.desktop_rect.width = 100.0;
    let mut b = surface(11, 2, 2.0, 0.0, 1.0, 1.0);
    b.parent = Some(ObjectRef {
        kind: ObjectKind::Surface,
        id: 10,
    });
    b.opacity = 0.5;
    s.records.insert(10, Record::Surface(a));
    s.records.insert(11, Record::Surface(b));
    let f = renderer()
        .render_view(&s, Viewport::new(5, 1), &())
        .unwrap();
    pixel(&f, 0, 0, 0.0, 0.0, 0.0, 0.0);
    pixel(&f, 1, 0, 1.0, 0.0, 0.0, 1.0);
    pixel(&f, 2, 0, 0.5, 0.0, 0.5, 1.0);
    pixel(&f, 3, 0, 0.0, 0.0, 0.0, 0.0);
}
#[test]
fn rounded_rectangle_and_border_have_analytic_corner_and_hole_coverage() {
    let n = node(
        1,
        Primitive::RoundedRect {
            rect: rect(0.0, 0.0, 4.0, 4.0),
            radii: radius(2.0, 2.0),
            color: red(),
        },
    );
    let f = render(vec![n], 4, 4, &());
    pixel(&f, 0, 0, 0.25, 0.0, 0.0, 0.25);
    pixel(&f, 1, 1, 1.0, 0.0, 0.0, 1.0);
    let n = node(
        1,
        Primitive::Border {
            rect: rect(0.0, 0.0, 4.0, 4.0),
            radii: radius(0.0, 0.0),
            width: 1.0,
            color: red(),
        },
    );
    let f = render(vec![n], 4, 4, &());
    pixel(&f, 0, 1, 1.0, 0.0, 0.0, 1.0);
    pixel(&f, 1, 1, 0.0, 0.0, 0.0, 0.0);
    let n = node(
        1,
        Primitive::Border {
            rect: rect(0.0, 0.0, 1.0, 1.0),
            radii: radius(0.0, 0.0),
            width: 0.5,
            color: red(),
        },
    );
    pixel(&render(vec![n], 1, 1, &()), 0, 0, 1.0, 0.0, 0.0, 1.0);
    let n = node(
        1,
        Primitive::Border {
            rect: rect(0.0, 0.0, 1.0, 1.0),
            radii: radius(0.0, 0.0),
            width: 0.0,
            color: red(),
        },
    );
    pixel(&render(vec![n], 1, 1, &()), 0, 0, 0.0, 0.0, 0.0, 0.0);
}
#[test]
fn gradients_interpolate_premultiplied_components_and_pad() {
    let n = node(
        1,
        Primitive::LinearGradient {
            rect: rect(0.0, 0.0, 3.0, 1.0),
            start: point(1.0, 0.0),
            end: point(2.0, 0.0),
            stops: stops(color(0.5, 0.0, 0.0, 0.5), blue()),
            spread: GradientSpread::Pad,
        },
    );
    let f = render(vec![n], 3, 1, &());
    pixel(&f, 0, 0, 0.5, 0.0, 0.0, 0.5);
    pixel(&f, 1, 0, 0.25, 0.0, 0.5, 0.75);
    pixel(&f, 2, 0, 0.0, 0.0, 1.0, 1.0);
}
#[test]
fn duplicate_stops_are_right_continuous_at_exact_sample() {
    let n = node(
        1,
        Primitive::LinearGradient {
            rect: rect(0.0, 0.0, 1.0, 1.0),
            start: point(0.0, 0.0),
            end: point(1.0, 0.0),
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: red(),
                },
                GradientStop {
                    offset: 0.25,
                    color: red(),
                },
                GradientStop {
                    offset: 0.25,
                    color: blue(),
                },
                GradientStop {
                    offset: 1.0,
                    color: blue(),
                },
            ],
            spread: GradientSpread::Pad,
        },
    );
    pixel(&render(vec![n], 1, 1, &()), 0, 0, 0.0, 0.0, 1.0, 1.0);
}
#[test]
fn radial_gradient_uses_euclidean_distance() {
    let n = node(
        1,
        Primitive::RadialGradient {
            rect: rect(0.0, 0.0, 1.0, 1.0),
            center: point(0.0, 0.0),
            inner_radius: 0.0,
            outer_radius: 2.0,
            stops: stops(red(), blue()),
            spread: GradientSpread::Pad,
        },
    );
    let mean =
        ((0.25f64.hypot(0.25) + 2.0 * 0.25f64.hypot(0.75) + 0.75f64.hypot(0.75)) / 4.0) / 2.0;
    pixel(
        &render(vec![n], 1, 1, &()),
        0,
        0,
        1.0 - mean,
        0.0,
        mean,
        1.0,
    );
}
#[test]
fn path_fill_rules_and_open_contour_closure() {
    let mut resources = Resources::default();
    let mut c = box_path(0.0, 0.0, 4.0, 4.0);
    c.extend(box_path(1.0, 1.0, 2.0, 2.0));
    let id = resources.add(ResourceKind::VectorPath, &c);
    pixel(
        &render(
            vec![path(1, id, FillRule::Nonzero, red())],
            4,
            4,
            &resources,
        ),
        1,
        1,
        1.0,
        0.0,
        0.0,
        1.0,
    );
    pixel(
        &render(
            vec![path(1, id, FillRule::EvenOdd, red())],
            4,
            4,
            &resources,
        ),
        1,
        1,
        0.0,
        0.0,
        0.0,
        0.0,
    );
    let id = resources.add(
        ResourceKind::VectorPath,
        &[
            Command::Move(0.0, 0.0),
            Command::Line(2.0, 0.0),
            Command::Line(0.0, 2.0),
        ],
    );
    pixel(
        &render(
            vec![path(1, id, FillRule::Nonzero, red())],
            2,
            2,
            &resources,
        ),
        0,
        0,
        1.0,
        0.0,
        0.0,
        1.0,
    );
    pixel(
        &render(
            vec![path(1, id, FillRule::Nonzero, red())],
            2,
            2,
            &resources,
        ),
        1,
        1,
        0.0,
        0.0,
        0.0,
        0.0,
    );
}
#[test]
fn quadratic_and_cubic_curves_preserve_known_interior_exterior_points() {
    let mut resources = Resources::default();
    for curve in [
        Command::Quad(2.0, 0.0, 4.0, 4.0),
        Command::Cubic(1.0, 0.0, 3.0, 0.0, 4.0, 4.0),
    ] {
        let id = resources.add(
            ResourceKind::VectorPath,
            &[Command::Move(0.0, 4.0), curve, Command::Close],
        );
        let f = render(
            vec![path(1, id, FillRule::Nonzero, red())],
            4,
            5,
            &resources,
        );
        pixel(&f, 1, 3, 1.0, 0.0, 0.0, 1.0);
        pixel(&f, 1, 0, 0.0, 0.0, 0.0, 0.0);
        pixel(&f, 1, 4, 0.0, 0.0, 0.0, 0.0);
    }
}
#[test]
fn stroke_caps_match_geometric_extensions_and_zero_length_is_empty() {
    let mut resources = Resources::default();
    let id = resources.add(
        ResourceKind::VectorPath,
        &[Command::Move(2.0, 2.0), Command::Line(4.0, 2.0)],
    );
    let butt = render(
        vec![stroke(
            1,
            id,
            2.0,
            StrokeCap::Butt,
            StrokeJoin::Bevel,
            vec![],
            0.0,
        )],
        6,
        4,
        &resources,
    );
    let round = render(
        vec![stroke(
            1,
            id,
            2.0,
            StrokeCap::Round,
            StrokeJoin::Bevel,
            vec![],
            0.0,
        )],
        6,
        4,
        &resources,
    );
    let square = render(
        vec![stroke(
            1,
            id,
            2.0,
            StrokeCap::Square,
            StrokeJoin::Bevel,
            vec![],
            0.0,
        )],
        6,
        4,
        &resources,
    );
    pixel(&butt, 1, 1, 0.0, 0.0, 0.0, 0.0);
    pixel(&round, 1, 1, 0.75, 0.0, 0.0, 0.75);
    pixel(&square, 1, 1, 1.0, 0.0, 0.0, 1.0);
    pixel(&butt, 2, 1, 1.0, 0.0, 0.0, 1.0);
    let id = resources.add(
        ResourceKind::VectorPath,
        &[Command::Move(2.0, 2.0), Command::Line(2.0, 2.0)],
    );
    let f = render(
        vec![stroke(
            1,
            id,
            2.0,
            StrokeCap::Round,
            StrokeJoin::Round,
            vec![],
            0.0,
        )],
        4,
        4,
        &resources,
    );
    assert!(f.pixels().iter().all(|p| p.alpha == 0.0));
}
#[test]
fn dash_phase_resets_each_contour_and_closing_segment_is_painted() {
    let mut resources = Resources::default();
    let id = resources.add(
        ResourceKind::VectorPath,
        &[
            Command::Move(0.0, 0.5),
            Command::Line(4.0, 0.5),
            Command::Move(0.0, 1.5),
            Command::Line(4.0, 1.5),
        ],
    );
    let f = render(
        vec![stroke(
            1,
            id,
            0.75,
            StrokeCap::Butt,
            StrokeJoin::Bevel,
            vec![1.0, 1.0],
            0.0,
        )],
        4,
        2,
        &resources,
    );
    for y in 0..2 {
        pixel(&f, 0, y, 1.0, 0.0, 0.0, 1.0);
        pixel(&f, 1, y, 0.0, 0.0, 0.0, 0.0);
        pixel(&f, 2, y, 1.0, 0.0, 0.0, 1.0);
    }
    let id = resources.add(ResourceKind::VectorPath, &box_path(1.0, 1.0, 4.0, 4.0));
    let f = render(
        vec![stroke(
            1,
            id,
            2.0,
            StrokeCap::Butt,
            StrokeJoin::Miter,
            vec![6.0, 2.0],
            1.0,
        )],
        6,
        6,
        &resources,
    );
    // Length16: pattern is painted on both sides of seam at top-left.
    // The closed dash merges there with a miter, reaching its outside corner.
    pixel(&f, 0, 0, 1.0, 0.0, 0.0, 1.0);
    pixel(&f, 0, 1, 1.0, 0.0, 0.0, 1.0);
}
#[test]
fn miter_join_falls_back_to_bevel_above_ratio_limit() {
    let mut resources = Resources::default();
    let id = resources.add(
        ResourceKind::VectorPath,
        &[
            Command::Move(1.0, 4.0),
            Command::Line(1.0, 1.0),
            Command::Line(4.0, 1.0),
        ],
    );
    let full = stroke(1, id, 2.0, StrokeCap::Butt, StrokeJoin::Miter, vec![], 0.0);
    let mut limited = full.clone();
    if let Primitive::StrokePath { miter_limit, .. } = &mut limited.primitive {
        *miter_limit = 1.0;
    }
    pixel(
        &render(vec![full], 5, 5, &resources),
        0,
        0,
        1.0,
        0.0,
        0.0,
        1.0,
    );
    // Right angle bevel cuts along x+y=1. Samples (.25,.25) outside; other
    // three on/inside the diagonal. Cairo pins exact boundary inclusion.
    pixel(
        &render(vec![limited], 5, 5, &resources),
        0,
        0,
        0.75,
        0.0,
        0.0,
        0.75,
    );
}
#[test]
fn glyph_positions_size_and_ordered_overlap_are_preserved() {
    let mut resources = Resources::default();
    let id = resources.add(ResourceKind::GlyphOutline, &box_path(0.0, -1.0, 1.0, 1.0));
    let n = node(
        1,
        Primitive::GlyphRun {
            font: GlyphFont::Outline(id),
            size: 2.0,
            glyph_ids: vec![0, 0],
            positions: vec![point(1.0, 3.0), point(1.0, 3.0)],
            scalar_clusters: vec![0, 1],
            color: color(0.5, 0.0, 0.0, 0.5),
        },
    );
    let f = render(vec![n], 4, 4, &resources);
    pixel(&f, 1, 1, 0.75, 0.0, 0.0, 0.75);
    pixel(&f, 0, 1, 0.0, 0.0, 0.0, 0.0);
    pixel(&f, 1, 3, 0.0, 0.0, 0.0, 0.0);
}
#[test]
fn resource_and_font_errors_are_explicit_even_when_node_is_transparent() {
    let mut resources = Resources::default();
    let id = resources.add(ResourceKind::VectorPath, &box_path(0.0, 0.0, 1.0, 1.0));
    let s = state([path(1, id, FillRule::Nonzero, red())]);
    let r = renderer();
    let v = Viewport::new(1, 1);
    assert!(matches!(
        r.render_scene(&s, reference(1), v, &()),
        Err(RenderError::MissingResource(_))
    ));
    resources.0.get_mut(&id).unwrap().0.digest = [0; 32];
    assert!(matches!(
        r.render_scene(&s, reference(1), v, &resources),
        Err(RenderError::ResourceDescriptor)
    ));
    resources.0.get_mut(&id).unwrap().0.digest = id;
    resources.0.get_mut(&id).unwrap().1[0] = 0;
    assert!(matches!(
        r.render_scene(&s, reference(1), v, &resources),
        Err(RenderError::ResourceIntegrity)
    ));
    let n = node(
        1,
        Primitive::GlyphRun {
            font: GlyphFont::Fingerprint(id),
            size: 1.0,
            glyph_ids: vec![1],
            positions: vec![point(0.0, 0.0)],
            scalar_clusters: vec![],
            color: red(),
        },
    );
    let mut n = n;
    n.opacity = 0.0;
    assert!(matches!(
        r.render_scene(&state([n.clone()]), reference(1), v, &()),
        Err(RenderError::MissingFont)
    ));
    if let Primitive::GlyphRun { font, .. } = &mut n.primitive {
        *font = GlyphFont::Outline(id);
    }
    assert!(matches!(
        r.render_scene(&state([n]), reference(1), v, &()),
        Err(RenderError::UnsupportedGlyph)
    ));
}
#[test]
fn malformed_public_state_rejects_without_panicking() {
    let r = renderer();
    let v = Viewport::new(1, 1);
    for n in [
        node(
            1,
            Primitive::LinearGradient {
                rect: rect(0.0, 0.0, 1.0, 1.0),
                start: point(0.0, 0.0),
                end: point(0.0, 0.0),
                stops: stops(red(), blue()),
                spread: GradientSpread::Pad,
            },
        ),
        node(
            1,
            Primitive::RoundedRect {
                rect: rect(0.0, 0.0, 1.0, 1.0),
                radii: radius(f64::NAN, 1.0),
                color: red(),
            },
        ),
        solid(1, rect(0.0, 0.0, 1.0, 1.0), color(1.0, 0.0, 0.0, 0.5)),
        node(
            1,
            Primitive::RadialGradient {
                rect: rect(0.0, 0.0, 1.0, 1.0),
                center: point(0.0, 0.0),
                inner_radius: 2.0,
                outer_radius: 1.0,
                stops: stops(red(), blue()),
                spread: GradientSpread::Pad,
            },
        ),
    ] {
        assert!(matches!(
            r.render_scene(&state([n]), reference(1), v, &()),
            Err(RenderError::Invalid)
        ));
    }
}
#[test]
fn local_work_memory_curve_dash_and_frame_limits_release_reservations() {
    let budget = MemoryBudget::new(1 << 20);
    let limits = RenderLimits {
        max_work: 1,
        ..RenderLimits::default()
    };
    let r = Renderer::new(limits, budget.clone()).unwrap();
    assert!(matches!(
        r.render_scene(
            &state([solid(1, rect(0.0, 0.0, 1.0, 1.0), red())]),
            reference(1),
            Viewport::new(1, 1),
            &()
        ),
        Err(RenderError::Limit(_))
    ));
    assert_eq!(budget.used(), 0);
    let r = Renderer::new(RenderLimits::default(), budget.clone()).unwrap();
    let f = r
        .render_scene(
            &state([solid(1, rect(0.0, 0.0, 1.0, 1.0), red())]),
            reference(1),
            Viewport::new(1, 1),
            &(),
        )
        .unwrap();
    assert_eq!(budget.used(), 32 + 256);
    drop(f);
    assert_eq!(budget.used(), 0);
    assert!(matches!(
        r.render_scene(
            &state([solid(1, rect(0.0, 0.0, 1.0, 1.0), red())]),
            reference(1),
            Viewport::new(4096, 4096),
            &()
        ),
        Err(RenderError::Limit(_))
    ));
    assert_eq!(budget.used(), 0);
    let mut resources = Resources::default();
    let id = resources.add(
        ResourceKind::VectorPath,
        &[
            Command::Move(0.0, 0.0),
            Command::Cubic(0.0, 100.0, 100.0, 100.0, 100.0, 0.0),
        ],
    );
    let limits = RenderLimits {
        max_curve_depth: 0,
        ..RenderLimits::default()
    };
    let r = Renderer::new(limits, budget.clone()).unwrap();
    assert!(matches!(
        r.render_scene(
            &state([path(1, id, FillRule::Nonzero, red())]),
            reference(1),
            Viewport::new(1, 1),
            &resources
        ),
        Err(RenderError::Limit("curve subdivision depth"))
    ));
    assert_eq!(budget.used(), 0);
    let n = stroke(
        1,
        id,
        1.0,
        StrokeCap::Butt,
        StrokeJoin::Bevel,
        vec![1e-12, 1e-12],
        0.0,
    );
    let r = Renderer::new(RenderLimits::default(), budget.clone()).unwrap();
    assert!(matches!(
        r.render_scene(&state([n]), reference(1), Viewport::new(1, 1), &resources),
        Err(RenderError::Limit("dash intervals"))
    ));
    assert_eq!(budget.used(), 0);
    let budget = MemoryBudget::new(100);
    let r = Renderer::new(RenderLimits::default(), budget.clone()).unwrap();
    assert!(matches!(
        r.render_scene(
            &state([solid(1, rect(0.0, 0.0, 1.0, 1.0), red())]),
            reference(1),
            Viewport::new(1, 1),
            &()
        ),
        Err(RenderError::Memory(_))
    ));
    assert_eq!(budget.used(), 0);
}
#[test]
fn partial_surfaces_and_cursor_shapes_cannot_silently_disappear() {
    let mut s = state([solid(1, rect(0.0, 0.0, 1.0, 1.0), red())]);
    let mut a = surface(10, 1, 0.0, 0.0, 1.0, 1.0);
    a.visual_coverage = VisualCoverage::Partial;
    s.records.insert(10, Record::Surface(a));
    assert!(matches!(
        renderer().render_view(&s, Viewport::new(1, 1), &()),
        Err(RenderError::IncompleteSurface(10))
    ));
    s.records.remove(&10);
    s.records.insert(
        11,
        Record::Session(SessionState {
            id: 11,
            version: 1,
            canvas: rect(0.0, 0.0, 1.0, 1.0),
            cursor: Some(Cursor {
                position: point(0.0, 0.0),
                hotspot: point(0.0, 0.0),
                appearance: CursorAppearance::Shape(CursorShape::Default),
            }),
            focus: None,
            focus_surface: None,
            focus_generation: 0,
        }),
    );
    assert!(matches!(
        renderer().render_view(&s, Viewport::new(1, 1), &()),
        Err(RenderError::UnsupportedCursor)
    ));
}

#[test]
fn malformed_strokes_and_safe_path_structure_fail_before_native_queries() {
    let mut resources = Resources::default();
    let id = resources.add(
        ResourceKind::VectorPath,
        &[Command::Move(0.0, 0.5), Command::Line(1.0, 0.5)],
    );
    let r = renderer();
    let v = Viewport::new(1, 1);
    for (width, dashes, offset, miter) in [
        (f64::NAN, vec![], 0.0, 1.0),
        (-1.0, vec![], 0.0, 1.0),
        (1.0, vec![0.0, 1.0], 0.0, 1.0),
        (1.0, vec![1.0], 0.0, 1.0),
        (1.0, vec![1.0, 1.0], 2.0, 1.0),
        (1.0, vec![], 1.0, 1.0),
        (1.0, vec![], 0.0, 2.0),
    ] {
        let mut n = stroke(
            1,
            id,
            width,
            StrokeCap::Butt,
            StrokeJoin::Bevel,
            dashes,
            offset,
        );
        if let Primitive::StrokePath { miter_limit, .. } = &mut n.primitive {
            *miter_limit = miter;
        }
        assert!(matches!(
            r.render_scene(&state([n]), reference(1), v, &resources),
            Err(RenderError::Invalid)
        ));
    }
    // Correct descriptor/hash cannot authorize malformed contour structure.
    let id = resources.add(ResourceKind::VectorPath, &[Command::Line(1.0, 1.0)]);
    assert!(matches!(
        r.render_scene(
            &state([path(1, id, FillRule::Nonzero, red())]),
            reference(1),
            v,
            &resources
        ),
        Err(RenderError::Resource(ResourceError::InvalidVector))
    ));
}

#[test]
fn native_stroke_overlaps_are_one_paint_union() {
    let mut resources = Resources::default();
    let id = resources.add(
        ResourceKind::VectorPath,
        &[
            Command::Move(0.0, 1.5),
            Command::Line(3.0, 1.5),
            Command::Move(1.5, 0.0),
            Command::Line(1.5, 3.0),
        ],
    );
    let mut n = stroke(1, id, 1.0, StrokeCap::Butt, StrokeJoin::Round, vec![], 0.0);
    if let Primitive::StrokePath { color, .. } = &mut n.primitive {
        *color = common::color(0.5, 0.0, 0.0, 0.5);
    }
    pixel(&render(vec![n], 3, 3, &resources), 1, 1, 0.5, 0.0, 0.0, 0.5);
}

#[test]
fn scene_graph_errors_reject_atomically() {
    let r = renderer();
    let v = Viewport::new(1, 1);
    for s in [
        state([group(1, &[1])]),
        state([group(1, &[2, 2]), solid(2, rect(0.0, 0.0, 1.0, 1.0), red())]),
        state([group(1, &[42])]),
    ] {
        assert!(matches!(
            r.render_scene(&s, reference(1), v, &()),
            Err(RenderError::Invalid)
        ));
    }
    let mut n = solid(1, rect(0.0, 0.0, 1.0, 1.0), red());
    n.children.push(reference(2));
    assert!(matches!(
        r.render_scene(&state([n]), reference(1), v, &()),
        Err(RenderError::Invalid)
    ));
}

#[test]
fn export_reserves_before_writing_and_frame_stays_owned_under_pressure() {
    let budget = MemoryBudget::new(1 << 20);
    let r = Renderer::new(RenderLimits::default(), budget.clone()).unwrap();
    let f = r
        .render_scene(
            &state([solid(1, rect(0.0, 0.0, 1.0, 1.0), red())]),
            reference(1),
            Viewport::new(1, 1),
            &(),
        )
        .unwrap();
    let pressure = budget.reserve(budget.limit() - budget.used()).unwrap();
    let mut bytes = vec![];
    assert!(matches!(
        f.write_pam(&mut bytes),
        Err(RenderError::Memory(_))
    ));
    assert!(bytes.is_empty());
    pixel(&f, 0, 0, 1.0, 0.0, 0.0, 1.0);
    drop(pressure);
    f.write_pam(&mut bytes).unwrap();
    drop(f);
    assert_eq!(budget.used(), 0);
}

#[test]
fn session_canvas_and_surface_transform_clip_the_absolute_desktop() {
    let mut s = state([solid(1, rect(0.0, 0.0, 10.0, 10.0), red())]);
    let mut a = surface(10, 1, 1.0, 0.0, 2.0, 2.0);
    a.transform.xx = 2.0;
    a.clip = Some(rect(0.0, 0.0, 1.0, 2.0));
    s.records.insert(10, Record::Surface(a));
    s.records.insert(
        11,
        Record::Session(SessionState {
            id: 11,
            version: 1,
            canvas: rect(0.0, 0.0, 2.0, 1.0),
            cursor: None,
            focus: None,
            focus_surface: None,
            focus_generation: 0,
        }),
    );
    let f = renderer()
        .render_view(&s, Viewport::new(4, 2), &())
        .unwrap();
    pixel(&f, 1, 0, 1.0, 0.0, 0.0, 1.0);
    pixel(&f, 2, 0, 0.0, 0.0, 0.0, 0.0);
    pixel(&f, 1, 1, 0.0, 0.0, 0.0, 0.0);
}
