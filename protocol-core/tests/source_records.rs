use rui_protocol_core::{
    model::{Limits, ModelError, State},
    wire::proto as p,
};
fn group(id: u64) -> p::Record {
    p::Record {
        record: Some(p::record::Record::Semantic(p::SemanticNode {
            id,
            version: 1,
            role: p::SemanticRole::GroupRole as i32,
            sensitivity: p::Sensitivity::Public as i32,
            completeness: p::Completeness::Full as i32,
            ..Default::default()
        })),
    }
}
#[test]
fn trusted_source_records_still_validate_identity_graph_and_numeric_limits() {
    let limits = Limits::default();
    assert!(State::from_source_records(0, vec![], &limits).is_err());
    assert!(matches!(
        State::from_source_records(1, vec![group(1), group(1)], &limits),
        Err(ModelError::DuplicateId(1))
    ));
    let mut dangling = group(1);
    let Some(p::record::Record::Semantic(n)) = &mut dangling.record else {
        panic!()
    };
    n.ordered_children.push(p::ObjectRef {
        kind: p::ObjectKind::Semantic as i32,
        id: 2,
    });
    assert!(State::from_source_records(1, vec![dangling], &limits).is_err());
    let state = State::from_source_records(4, vec![group(1)], &limits).unwrap();
    assert_eq!(state.revision, 4);
    assert!(state.retired.is_empty());
    let tiny = Limits {
        max_memory_bytes: 1,
        ..limits.clone()
    };
    assert!(matches!(
        State::from_source_records(1, vec![group(1)], &tiny),
        Err(ModelError::Limit("decoded source state"))
    ));
    let tiny = Limits {
        max_records: 1,
        ..limits.clone()
    };
    assert!(matches!(
        State::from_source_records(1, vec![group(1), group(2)], &tiny),
        Err(ModelError::Limit("records"))
    ));
    let invalid = p::Record {
        record: Some(p::record::Record::Scene(p::SceneNode {
            id: 2,
            version: 1,
            transform: Some(p::Affine {
                xx: 1.0,
                yy: 1.0,
                ..Default::default()
            }),
            opacity: 1.0,
            primitive: Some(p::scene_node::Primitive::SolidRect(p::SolidRect {
                rect: Some(p::Rect {
                    x: f64::NAN,
                    y: 0.0,
                    width: 1.0,
                    height: 1.0,
                }),
                color: Some(p::Color {
                    alpha: 1.0,
                    ..Default::default()
                }),
            })),
            ..Default::default()
        })),
    };
    assert!(State::from_source_records(1, vec![invalid], &limits).is_err());
}
