use prost::Message;
use rui_protocol_core::model::{
    ApplyOutcome, GradientStop, Limits, ModelError, Primitive, Record, Replica, ResourceId,
    StrokeCap, StrokeJoin,
};
use rui_protocol_core::resources::MemoryBudget;
use rui_protocol_core::wire::{self, proto as p, WireError};
use std::collections::BTreeSet;

fn bounds() -> p::Rect {
    p::Rect {
        x: 10.0,
        y: 20.0,
        width: 100.0,
        height: 80.0,
    }
}
fn color() -> p::Color {
    p::Color {
        red: 0.5,
        green: 0.25,
        blue: 0.0,
        alpha: 0.75,
    }
}
fn radii() -> p::CornerRadii {
    let r = Some(p::Size {
        width: 8.0,
        height: 6.0,
    });
    p::CornerRadii {
        top_left: r,
        top_right: r,
        bottom_right: r,
        bottom_left: r,
    }
}
fn stops() -> Vec<p::GradientStop> {
    vec![
        p::GradientStop {
            offset: 0.0,
            color: Some(color()),
        },
        p::GradientStop {
            offset: 0.5,
            color: Some(p::Color {
                red: 0.5,
                green: 0.0,
                blue: 0.0,
                alpha: 0.5,
            }),
        },
        p::GradientStop {
            offset: 0.5,
            color: Some(p::Color {
                red: 0.0,
                green: 0.0,
                blue: 0.5,
                alpha: 0.5,
            }),
        },
        p::GradientStop {
            offset: 1.0,
            color: Some(color()),
        },
    ]
}
fn rounded() -> p::RoundedRect {
    p::RoundedRect {
        rect: Some(bounds()),
        radii: Some(radii()),
        color: Some(color()),
    }
}
fn border() -> p::Border {
    p::Border {
        rect: Some(bounds()),
        radii: Some(radii()),
        width: 2.0,
        color: Some(color()),
    }
}
fn linear() -> p::LinearGradient {
    p::LinearGradient {
        rect: Some(bounds()),
        start: Some(p::Point { x: 0.0, y: 0.0 }),
        end: Some(p::Point { x: 100.0, y: 0.0 }),
        stops: stops(),
        spread: p::GradientSpread::Pad as i32,
    }
}
fn radial() -> p::RadialGradient {
    p::RadialGradient {
        rect: Some(bounds()),
        center: Some(p::Point { x: 50.0, y: 40.0 }),
        inner_radius: 10.0,
        outer_radius: 40.0,
        stops: stops(),
        spread: p::GradientSpread::Pad as i32,
    }
}
fn stroke() -> p::StrokePath {
    p::StrokePath {
        resource: vec![9; 32],
        color: Some(color()),
        width: 4.0,
        cap: p::StrokeCap::Square as i32,
        join: p::StrokeJoin::Miter as i32,
        miter_limit: 4.0,
        dash_lengths: vec![3.0, 2.0],
        dash_offset: 1.0,
    }
}
fn all() -> Vec<p::scene_node::Primitive> {
    vec![
        p::scene_node::Primitive::Group(p::Group {}),
        p::scene_node::Primitive::SolidRect(p::SolidRect {
            rect: Some(bounds()),
            color: Some(color()),
        }),
        p::scene_node::Primitive::FillPath(p::FillPath {
            resource: vec![9; 32],
            color: Some(color()),
            fill_rule: p::FillRule::EvenOdd as i32,
        }),
        p::scene_node::Primitive::GlyphRun(p::GlyphRun {
            font: Some(p::glyph_run::Font::FontFingerprint(vec![8; 32])),
            size: 12.0,
            glyph_ids: vec![1, 2],
            positions: vec![p::Point { x: 0.0, y: 12.0 }, p::Point { x: 8.0, y: 12.0 }],
            scalar_clusters: vec![0, 1],
            color: Some(color()),
        }),
        p::scene_node::Primitive::RoundedRect(rounded()),
        p::scene_node::Primitive::Border(border()),
        p::scene_node::Primitive::LinearGradient(linear()),
        p::scene_node::Primitive::RadialGradient(radial()),
        p::scene_node::Primitive::StrokePath(stroke()),
    ]
}
fn available() -> BTreeSet<ResourceId> {
    BTreeSet::from([[8; 32], [9; 32]])
}
fn scene(id: u64, version: u64, primitive: Option<p::scene_node::Primitive>) -> p::SceneNode {
    p::SceneNode {
        id,
        version,
        transform: Some(p::Affine {
            xx: 1.0,
            yy: 1.0,
            ..Default::default()
        }),
        opacity: 1.0,
        primitive,
        ..Default::default()
    }
}
fn wrap_scene(node: p::SceneNode) -> p::Record {
    p::Record {
        record: Some(p::record::Record::Scene(node)),
    }
}
fn snapshot(primitive: p::scene_node::Primitive) -> Vec<u8> {
    p::Snapshot {
        revision: 1,
        records: vec![
            wrap_scene(scene(1, 1, Some(primitive))),
            p::Record {
                record: Some(p::record::Record::Semantic(label(1, "old"))),
            },
        ],
        ..Default::default()
    }
    .encode_to_vec()
}
fn label(version: u64, text: &str) -> p::SemanticNode {
    p::SemanticNode {
        id: 99,
        version,
        role: p::SemanticRole::Label as i32,
        label: Some(text.into()),
        sensitivity: p::Sensitivity::Public as i32,
        completeness: p::Completeness::Full as i32,
        ..Default::default()
    }
}
fn put(record: p::Record) -> p::Mutation {
    p::Mutation {
        mutation: Some(p::mutation::Mutation::Put(record)),
    }
}
fn loaded() -> Replica {
    let mut replica = Replica::new(Limits::default());
    replica
        .apply_snapshot(
            &snapshot(p::scene_node::Primitive::Group(p::Group {})),
            &available(),
        )
        .unwrap();
    replica
}
fn reject(primitive: p::scene_node::Primitive) -> ModelError {
    let mut replica = loaded();
    let before = replica.state().clone();
    let bytes = p::Transaction {
        base_revision: 1,
        new_revision: 2,
        mutations: vec![
            put(p::Record {
                record: Some(p::record::Record::Semantic(label(2, "new"))),
            }),
            put(wrap_scene(scene(1, 2, Some(primitive)))),
        ],
        ..Default::default()
    }
    .encode_to_vec();
    let error = replica.apply_transaction(&bytes, &available()).unwrap_err();
    assert_eq!(replica.state(), &before);
    error
}
fn accepted(primitive: p::scene_node::Primitive) -> Replica {
    let mut replica = Replica::new(Limits::default());
    replica
        .apply_snapshot(&snapshot(primitive), &available())
        .unwrap();
    replica
}
fn retained(replica: &Replica) -> &Primitive {
    let Record::Scene(node) = &replica.state().records[&1] else {
        panic!()
    };
    &node.primitive
}

#[test]
fn all_nine_primitives_roundtrip_protobuf_and_commit_typed_records() {
    let records = all()
        .into_iter()
        .enumerate()
        .map(|(i, v)| wrap_scene(scene(i as u64 + 1, 1, Some(v))))
        .collect();
    let message = p::Snapshot {
        revision: 1,
        records,
        ..Default::default()
    };
    let bytes = message.encode_to_vec();
    assert_eq!(
        wire::decode_snapshot(&bytes, &Limits::default()).unwrap(),
        message
    );
    let mut replica = Replica::new(Limits::default());
    assert_eq!(
        replica.apply_snapshot(&bytes, &available()).unwrap(),
        ApplyOutcome::Applied { revision: 1 }
    );
    assert_eq!(replica.state().records.len(), 9);
    assert_eq!(replica.resource_refs(), &available());
    let kinds = replica
        .state()
        .records
        .values()
        .map(|record| {
            let Record::Scene(scene) = record else {
                panic!()
            };
            match scene.primitive {
                Primitive::Group => "Group",
                Primitive::SolidRect { .. } => "SolidRect",
                Primitive::FillPath { .. } => "FillPath",
                Primitive::GlyphRun { .. } => "GlyphRun",
                Primitive::RoundedRect { .. } => "RoundedRect",
                Primitive::Border { .. } => "Border",
                Primitive::LinearGradient { .. } => "LinearGradient",
                Primitive::RadialGradient { .. } => "RadialGradient",
                Primitive::StrokePath { .. } => "StrokePath",
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            "Group",
            "SolidRect",
            "FillPath",
            "GlyphRun",
            "RoundedRect",
            "Border",
            "LinearGradient",
            "RadialGradient",
            "StrokePath"
        ]
    );
}

#[test]
fn rounded_corner_radii_reject_negative_nonfinite_extreme_and_mixed_zero() {
    for radius in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 2e9, 0.0] {
        let mut v = rounded();
        v.radii.as_mut().unwrap().top_left.as_mut().unwrap().width = radius;
        assert!(matches!(
            reject(p::scene_node::Primitive::RoundedRect(v)),
            ModelError::Invalid(_)
        ));
    }
    let mut v = rounded();
    v.radii.as_mut().unwrap().bottom_right = None;
    assert!(matches!(
        reject(p::scene_node::Primitive::RoundedRect(v)),
        ModelError::Invalid(_)
    ));
}

#[test]
fn corner_edge_sums_are_rejected_without_implicit_scaling() {
    let mut v = rounded();
    let r = v.radii.as_mut().unwrap();
    r.top_left.as_mut().unwrap().width = 60.0;
    r.top_right.as_mut().unwrap().width = 60.0;
    reject(p::scene_node::Primitive::RoundedRect(v));
    let mut v = rounded();
    let r = v.radii.as_mut().unwrap();
    r.top_left.as_mut().unwrap().height = 50.0;
    r.bottom_left.as_mut().unwrap().height = 50.0;
    reject(p::scene_node::Primitive::RoundedRect(v));
    let mut v = rounded();
    v.radii = Some(p::CornerRadii {
        top_left: Some(p::Size {
            width: 50.0,
            height: 40.0,
        }),
        top_right: Some(p::Size {
            width: 50.0,
            height: 40.0,
        }),
        bottom_right: Some(p::Size {
            width: 50.0,
            height: 40.0,
        }),
        bottom_left: Some(p::Size {
            width: 50.0,
            height: 40.0,
        }),
    });
    accepted(p::scene_node::Primitive::RoundedRect(v));
}

#[test]
fn border_width_is_uniform_inward_bounded_and_zero_is_valid() {
    for width in [-1.0, 41.0, f64::NAN, f64::INFINITY, 2e9] {
        let mut v = border();
        v.width = width;
        reject(p::scene_node::Primitive::Border(v));
    }
    let mut v = border();
    v.width = 0.0;
    accepted(p::scene_node::Primitive::Border(v));
    v.width = 40.0;
    accepted(p::scene_node::Primitive::Border(v));
}

#[test]
fn border_checks_derived_inner_corner_fit_and_collapsed_components() {
    let mut v = border();
    v.width = 10.0;
    let zero = Some(p::Size {
        width: 0.0,
        height: 0.0,
    });
    v.radii = Some(p::CornerRadii {
        top_left: Some(p::Size {
            width: 100.0,
            height: 40.0,
        }),
        top_right: zero,
        bottom_right: zero,
        bottom_left: zero,
    });
    assert!(matches!(
        reject(p::scene_node::Primitive::Border(v)),
        ModelError::Invalid(_)
    ));
    let mut v = border();
    v.width = 7.0;
    accepted(p::scene_node::Primitive::Border(v));
}

#[test]
fn gradient_duplicate_stops_retain_transmitted_order_and_negative_zero_normalizes() {
    let mut v = linear();
    v.stops[0].offset = -0.0;
    let replica = accepted(p::scene_node::Primitive::LinearGradient(v));
    let Primitive::LinearGradient { stops, .. } = retained(&replica) else {
        panic!()
    };
    assert_eq!(stops[0].offset.to_bits(), 0);
    assert_eq!(stops[1].offset, stops[2].offset);
    assert_eq!(stops[1].color.red, 0.5);
    assert_eq!(stops[2].color.blue, 0.5);
}

#[test]
fn gradients_reject_unsorted_missing_endpoint_and_invalid_offsets() {
    let mut cases = Vec::new();
    let mut v = linear();
    v.stops[1].offset = 0.75;
    v.stops[2].offset = 0.25;
    cases.push(v);
    let mut v = linear();
    v.stops[0].offset = 0.1;
    cases.push(v);
    let mut v = linear();
    v.stops[3].offset = 0.9;
    cases.push(v);
    let mut v = linear();
    v.stops.truncate(1);
    cases.push(v);
    for value in [-0.1, 1.1, f64::NAN, f64::INFINITY] {
        let mut v = linear();
        v.stops[1].offset = value;
        cases.push(v);
    }
    for v in cases {
        reject(p::scene_node::Primitive::LinearGradient(v));
    }
}

#[test]
fn gradients_reject_nonfinite_degenerate_geometry_and_unknown_spread() {
    let mut v = linear();
    v.end = v.start;
    reject(p::scene_node::Primitive::LinearGradient(v));
    for value in [f64::NAN, f64::INFINITY, 2e9] {
        let mut v = linear();
        v.end.as_mut().unwrap().x = value;
        reject(p::scene_node::Primitive::LinearGradient(v));
    }
    for spread in [0, 2, 99] {
        let mut v = linear();
        v.spread = spread;
        assert_eq!(
            reject(p::scene_node::Primitive::LinearGradient(v)),
            ModelError::Unsupported
        );
        let mut v = radial();
        v.spread = spread;
        assert_eq!(
            reject(p::scene_node::Primitive::RadialGradient(v)),
            ModelError::Unsupported
        );
    }
}

#[test]
fn radial_circles_are_concentric_positive_ordered_and_have_bounded_extrema() {
    for inner in [-1.0, 40.0, 41.0, f64::NAN, f64::INFINITY] {
        let mut v = radial();
        v.inner_radius = inner;
        reject(p::scene_node::Primitive::RadialGradient(v));
    }
    for outer in [0.0, -1.0, 10.0, f64::NAN, f64::INFINITY, 2e9] {
        let mut v = radial();
        v.outer_radius = outer;
        reject(p::scene_node::Primitive::RadialGradient(v));
    }
    let mut v = radial();
    v.center.as_mut().unwrap().x = 1e9;
    reject(p::scene_node::Primitive::RadialGradient(v));
    let mut v = radial();
    v.inner_radius = -0.0;
    let replica = accepted(p::scene_node::Primitive::RadialGradient(v));
    let Primitive::RadialGradient { inner_radius, .. } = retained(&replica) else {
        panic!()
    };
    assert_eq!(inner_radius.to_bits(), 0);
}

#[test]
fn each_new_primitive_rejects_nonpremultiplied_or_nonfinite_colors_atomically() {
    for invalid_color in [
        p::Color {
            red: 0.8,
            green: 0.0,
            blue: 0.0,
            alpha: 0.1,
        },
        p::Color {
            red: f64::NAN,
            ..color()
        },
        p::Color {
            alpha: f64::INFINITY,
            ..color()
        },
    ] {
        let mut v = rounded();
        v.color = Some(invalid_color);
        reject(p::scene_node::Primitive::RoundedRect(v));
        let mut v = border();
        v.color = Some(invalid_color);
        reject(p::scene_node::Primitive::Border(v));
        let mut v = linear();
        v.stops[1].color = Some(invalid_color);
        reject(p::scene_node::Primitive::LinearGradient(v));
        let mut v = radial();
        v.stops[1].color = Some(invalid_color);
        reject(p::scene_node::Primitive::RadialGradient(v));
        let mut v = stroke();
        v.color = Some(invalid_color);
        reject(p::scene_node::Primitive::StrokePath(v));
    }
}

#[test]
fn stroke_supports_every_cap_join_with_canonical_miter_options() {
    for cap in [
        p::StrokeCap::Butt,
        p::StrokeCap::RoundCap,
        p::StrokeCap::Square,
    ] {
        for join in [
            p::StrokeJoin::Miter,
            p::StrokeJoin::RoundJoin,
            p::StrokeJoin::Bevel,
        ] {
            let mut v = stroke();
            v.cap = cap as i32;
            v.join = join as i32;
            v.miter_limit = if join == p::StrokeJoin::Miter {
                4.0
            } else {
                1.0
            };
            let replica = accepted(p::scene_node::Primitive::StrokePath(v));
            let Primitive::StrokePath { cap, join, .. } = retained(&replica) else {
                panic!()
            };
            assert!(matches!(
                cap,
                StrokeCap::Butt | StrokeCap::Round | StrokeCap::Square
            ));
            assert!(matches!(
                join,
                StrokeJoin::Miter | StrokeJoin::Round | StrokeJoin::Bevel
            ));
        }
    }
}

#[test]
fn stroke_rejects_unknown_cap_join_nonfinite_width_and_unsupported_miter() {
    for cap in [0, 99] {
        let mut v = stroke();
        v.cap = cap;
        assert_eq!(
            reject(p::scene_node::Primitive::StrokePath(v)),
            ModelError::Unsupported
        );
    }
    for join in [0, 99] {
        let mut v = stroke();
        v.join = join;
        assert_eq!(
            reject(p::scene_node::Primitive::StrokePath(v)),
            ModelError::Unsupported
        );
    }
    for width in [-1.0, 0.0, f64::NAN, f64::INFINITY, 2e9] {
        let mut v = stroke();
        v.width = width;
        reject(p::scene_node::Primitive::StrokePath(v));
    }
    for limit in [0.5, 33.0, f64::NAN, f64::INFINITY] {
        let mut v = stroke();
        v.miter_limit = limit;
        reject(p::scene_node::Primitive::StrokePath(v));
    }
    let mut v = stroke();
    v.join = p::StrokeJoin::RoundJoin as i32;
    reject(p::scene_node::Primitive::StrokePath(v));
}

#[test]
fn stroke_dash_pattern_is_positive_even_bounded_and_offset_normalized() {
    for lengths in [
        vec![3.0],
        vec![3.0, 0.0],
        vec![3.0, -1.0],
        vec![3.0, f64::NAN],
        vec![3.0, f64::INFINITY],
        vec![1e9, 1e9],
    ] {
        let mut v = stroke();
        v.dash_lengths = lengths;
        v.dash_offset = 0.0;
        reject(p::scene_node::Primitive::StrokePath(v));
    }
    for offset in [-1.0, 5.0, 6.0, f64::NAN, f64::INFINITY] {
        let mut v = stroke();
        v.dash_offset = offset;
        reject(p::scene_node::Primitive::StrokePath(v));
    }
    let mut v = stroke();
    v.dash_lengths.clear();
    v.dash_offset = 1.0;
    reject(p::scene_node::Primitive::StrokePath(v.clone()));
    v.dash_offset = -0.0;
    let replica = accepted(p::scene_node::Primitive::StrokePath(v));
    let Primitive::StrokePath { dash_offset, .. } = retained(&replica) else {
        panic!()
    };
    assert_eq!(dash_offset.to_bits(), 0);
}

#[test]
fn stroke_resource_is_implicit_and_missing_closure_keeps_publication_staged() {
    let budget = MemoryBudget::new(1 << 20);
    let mut replica = Replica::with_memory_budget(Limits::default(), budget.clone());
    let bytes = snapshot(p::scene_node::Primitive::StrokePath(stroke()));
    let outcome = replica.apply_snapshot(&bytes, &BTreeSet::new()).unwrap();
    let ApplyOutcome::PendingResources { revision, missing } = outcome else {
        panic!()
    };
    assert_eq!(revision, 1);
    assert_eq!(*missing, BTreeSet::from([[9; 32]]));
    drop(missing);
    assert_eq!(replica.state().revision, 0);
    assert_eq!(
        replica.pending_resources(),
        Some(&BTreeSet::from([[9; 32]]))
    );
    replica.resume_pending(&BTreeSet::from([[9; 32]])).unwrap();
    assert_eq!(replica.resource_refs(), &BTreeSet::from([[9; 32]]));
    drop(replica);
    assert_eq!(budget.used(), 0);
    let mut v = stroke();
    v.resource = vec![9; 31];
    reject(p::scene_node::Primitive::StrokePath(v));
}

fn varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 128 {
        output.push((value as u8) | 128);
        value >>= 7;
    }
    output.push(value as u8);
}
fn field(tag: u64, bytes: &[u8], output: &mut Vec<u8>) {
    varint((tag << 3) | 2, output);
    varint(bytes.len() as u64, output);
    output.extend_from_slice(bytes);
}
fn raw_snapshot(scene_bytes: &[u8]) -> Vec<u8> {
    let mut record = Vec::new();
    field(2, scene_bytes, &mut record);
    let mut bytes = vec![8, 1];
    field(2, &record, &mut bytes);
    bytes
}
fn raw_primitive(tag: u64, primitive_bytes: &[u8]) -> Vec<u8> {
    let mut node = scene(1, 1, None).encode_to_vec();
    field(tag, primitive_bytes, &mut node);
    raw_snapshot(&node)
}

#[test]
fn stop_limit_is_checked_before_protobuf_allocation_including_repeated_singular_merge() {
    let limits = Limits {
        max_gradient_stops: 2,
        ..Limits::default()
    };
    let bytes = snapshot(p::scene_node::Primitive::LinearGradient(linear()));
    assert_eq!(
        wire::decode_snapshot(&bytes, &limits),
        Err(WireError::Limit)
    );
    let mut gradient = linear();
    gradient.stops = vec![stops()[0], stops()[3]];
    let gradient = gradient.encode_to_vec();
    let mut node = scene(1, 1, None).encode_to_vec();
    field(16, &gradient, &mut node);
    field(16, &gradient, &mut node);
    let bytes = raw_snapshot(&node);
    let decoded = p::Snapshot::decode(bytes.as_slice()).unwrap();
    let Some(p::record::Record::Scene(node)) = &decoded.records[0].record else {
        panic!()
    };
    let Some(p::scene_node::Primitive::LinearGradient(gradient)) = &node.primitive else {
        panic!()
    };
    assert_eq!(gradient.stops.len(), 4);
    assert_eq!(
        wire::decode_snapshot(&bytes, &limits),
        Err(WireError::Limit)
    );
}

#[test]
fn packed_unpacked_and_mixed_dash_encodings_share_preallocation_limits() {
    let limits = Limits {
        max_dash_segments: 2,
        ..Limits::default()
    };
    let mut v = stroke();
    v.dash_lengths = vec![1.0, 1.0, 1.0, 1.0];
    v.dash_offset = 0.0;
    assert_eq!(
        wire::decode_snapshot(
            &snapshot(p::scene_node::Primitive::StrokePath(v.clone())),
            &limits
        ),
        Err(WireError::Limit)
    );
    v.dash_lengths.clear();
    let base = v.encode_to_vec();
    let mut unpacked = base.clone();
    for _ in 0..4 {
        unpacked.push(57);
        unpacked.extend_from_slice(&1.0f64.to_le_bytes());
    }
    assert_eq!(
        wire::decode_snapshot(&raw_primitive(18, &unpacked), &limits),
        Err(WireError::Limit)
    );
    let mut mixed = base.clone();
    for _ in 0..2 {
        mixed.push(57);
        mixed.extend_from_slice(&1.0f64.to_le_bytes());
    }
    field(7, &1.0f64.to_le_bytes(), &mut mixed);
    assert_eq!(
        wire::decode_snapshot(&raw_primitive(18, &mixed), &limits),
        Err(WireError::Limit)
    );
    let mut malformed = base;
    field(7, &[0], &mut malformed);
    assert!(matches!(
        wire::decode_snapshot(&raw_primitive(18, &malformed), &limits),
        Err(WireError::Malformed(_))
    ));
}

#[test]
fn unknown_required_variants_and_options_fail_even_beside_known_variants() {
    for tag in [19, 99] {
        let mut node =
            scene(1, 1, Some(p::scene_node::Primitive::Group(p::Group {}))).encode_to_vec();
        field(tag, &[], &mut node);
        assert!(matches!(
            wire::decode_snapshot(&raw_snapshot(&node), &Limits::default()),
            Err(WireError::Unsupported(_))
        ));
    }
    let mut v = rounded().encode_to_vec();
    field(4, &[], &mut v);
    assert!(matches!(
        wire::decode_snapshot(&raw_primitive(14, &v), &Limits::default()),
        Err(WireError::Unsupported(_))
    ));
    let mut v = radial().encode_to_vec();
    field(7, &[], &mut v);
    assert!(matches!(
        wire::decode_snapshot(&raw_primitive(17, &v), &Limits::default()),
        Err(WireError::Unsupported(_))
    ));
    let mut v = rounded().encode_to_vec();
    field(128, &[1, 2, 3], &mut v);
    let mut replica = Replica::new(Limits::default());
    replica
        .apply_snapshot(&raw_primitive(14, &v), &BTreeSet::new())
        .unwrap();
    assert!(matches!(retained(&replica), Primitive::RoundedRect { .. }));
}

#[test]
fn retained_gradient_and_dash_capacity_is_included_in_state_accounting() {
    let base = accepted(p::scene_node::Primitive::Group(p::Group {}))
        .state()
        .estimated_bytes();
    let mut v = linear();
    v.stops = (0..1024)
        .map(|i| p::GradientStop {
            offset: i as f64 / 1023.0,
            color: Some(color()),
        })
        .collect();
    let replica = accepted(p::scene_node::Primitive::LinearGradient(v));
    let Primitive::LinearGradient { stops, .. } = retained(&replica) else {
        panic!()
    };
    assert_eq!(
        replica.state().estimated_bytes() - base,
        stops.capacity() * std::mem::size_of::<GradientStop>() + 32
    );
    let mut v = stroke();
    v.dash_lengths = vec![1.0; 1024];
    v.dash_offset = 0.0;
    let replica = accepted(p::scene_node::Primitive::StrokePath(v));
    let Primitive::StrokePath { dash_lengths, .. } = retained(&replica) else {
        panic!()
    };
    assert_eq!(
        replica.state().estimated_bytes() - base,
        dash_lengths.capacity() * std::mem::size_of::<f64>() + 32
    );
}

#[test]
fn configurable_local_limits_can_be_lowered_without_partial_state() {
    let mut replica = Replica::new(Limits {
        max_stroke_miter_limit: 2.0,
        ..Limits::default()
    });
    let bytes = snapshot(p::scene_node::Primitive::StrokePath(stroke()));
    assert!(replica.apply_snapshot(&bytes, &available()).is_err());
    assert_eq!(replica.state().revision, 0);
    let mut replica = Replica::new(Limits {
        max_records: 1,
        max_gradient_stops: 4096,
        ..Limits::default()
    });
    let mut v = linear();
    v.stops = (0..256)
        .map(|i| p::GradientStop {
            offset: i as f64 / 255.0,
            color: Some(color()),
        })
        .collect();
    replica
        .apply_snapshot(&raw_primitive(16, &v.encode_to_vec()), &BTreeSet::new())
        .unwrap();
}

#[test]
fn dimensionless_miter_ratio_uses_its_own_ceiling_with_small_geometry_limits() {
    let mut replica = Replica::new(Limits {
        max_geometry: 1.0,
        ..Limits::default()
    });
    let mut v = stroke();
    v.width = 0.01;
    v.miter_limit = 2.0;
    v.dash_lengths.clear();
    v.dash_offset = 0.0;
    replica
        .apply_snapshot(
            &snapshot(p::scene_node::Primitive::StrokePath(v)),
            &available(),
        )
        .unwrap();
    let Primitive::StrokePath {
        width, miter_limit, ..
    } = retained(&replica)
    else {
        panic!()
    };
    assert_eq!(*width, 0.01);
    assert_eq!(*miter_limit, 2.0);
    assert_eq!(replica.state().revision, 1);
}
