use prost::Message;
use rui_protocol_core::model::{ApplyOutcome, Limits, ModelError, Record, Replica, ResourceId};
use rui_protocol_core::resources::MemoryBudget;
use rui_protocol_core::wire::{self, proto as p, WireError};
use std::collections::BTreeSet;

fn reference(kind: p::ObjectKind, id: u64) -> p::ObjectRef {
    p::ObjectRef {
        kind: kind as i32,
        id,
    }
}
fn rect() -> p::Rect {
    p::Rect {
        x: 0.0,
        y: 0.0,
        width: 100.0,
        height: 100.0,
    }
}
fn affine() -> p::Affine {
    p::Affine {
        xx: 1.0,
        yy: 1.0,
        ..Default::default()
    }
}
fn color() -> p::Color {
    p::Color {
        red: 0.5,
        green: 0.25,
        blue: 0.0,
        alpha: 1.0,
    }
}
fn semantic(id: u64, children: &[u64]) -> p::SemanticNode {
    p::SemanticNode {
        id,
        version: 1,
        role: if id == 30 {
            p::SemanticRole::Root
        } else {
            p::SemanticRole::Button
        } as i32,
        label: Some(format!("node {id}")),
        sensitivity: p::Sensitivity::Public as i32,
        completeness: p::Completeness::Full as i32,
        ordered_children: children
            .iter()
            .map(|id| reference(p::ObjectKind::Semantic, *id))
            .collect(),
        ..Default::default()
    }
}
fn scene(id: u64) -> p::SceneNode {
    p::SceneNode {
        id,
        version: 1,
        transform: Some(affine()),
        opacity: 1.0,
        primitive: Some(p::scene_node::Primitive::Group(p::Group {})),
        ..Default::default()
    }
}
fn wrap(record: p::record::Record) -> p::Record {
    p::Record {
        record: Some(record),
    }
}
fn put(record: p::record::Record) -> p::Mutation {
    p::Mutation {
        mutation: Some(p::mutation::Mutation::Put(wrap(record))),
    }
}
fn delete(kind: p::ObjectKind, id: u64) -> p::Mutation {
    p::Mutation {
        mutation: Some(p::mutation::Mutation::Delete(reference(kind, id))),
    }
}
fn snapshot(revision: u64, records: Vec<p::Record>) -> Vec<u8> {
    p::Snapshot {
        revision,
        records,
        ..Default::default()
    }
    .encode_to_vec()
}
fn transaction(base: u64, mutations: Vec<p::Mutation>) -> Vec<u8> {
    p::Transaction {
        base_revision: base,
        new_revision: base + 1,
        mutations,
        ..Default::default()
    }
    .encode_to_vec()
}
fn empty_resources() -> BTreeSet<ResourceId> {
    BTreeSet::new()
}
fn fixture() -> Vec<p::Record> {
    let surface = p::Surface {
        id: 10,
        version: 1,
        role: p::SurfaceRole::Toplevel as i32,
        desktop_rect: Some(rect()),
        content_size: Some(p::Size {
            width: 100.0,
            height: 100.0,
        }),
        transform: Some(affine()),
        opacity: 1.0,
        mapped: true,
        scene_root: Some(reference(p::ObjectKind::Scene, 20)),
        semantic_root: Some(reference(p::ObjectKind::Semantic, 30)),
        input_region: vec![rect()],
        routing_token: b"route".to_vec(),
        visual_coverage: p::VisualCoverage::Complete as i32,
        semantic_coverage: p::SemanticCoverage::Rich as i32,
        capture_consistency: p::CaptureConsistency::Coherent as i32,
        source_capture_tokens: vec![b"source frame 1".to_vec()],
        ..Default::default()
    };
    let mut root_scene = scene(20);
    root_scene.ordered_children = vec![reference(p::ObjectKind::Scene, 21)];
    let mut leaf_scene = scene(21);
    leaf_scene.primitive = Some(p::scene_node::Primitive::SolidRect(p::SolidRect {
        rect: Some(rect()),
        color: Some(color()),
    }));
    let root = semantic(30, &[31, 32]);
    let mut button = semantic(31, &[]);
    button.label = Some("Save".into());
    button.description = Some("Write changes".into());
    button.typed_value = Some(p::TypedValue {
        value: Some(p::typed_value::Value::Boolean(false)),
    });
    button.visual_bindings = vec![reference(p::ObjectKind::Binding, 41)];
    button.actions = vec![p::Action {
        id: "Activate".into(),
        schema_version: 1,
    }];
    let mut entry = semantic(32, &[]);
    entry.role = p::SemanticRole::TextField as i32;
    entry.text_model = Some(reference(p::ObjectKind::Text, 40));
    let text = p::TextModel {
        id: 40,
        version: 1,
        text_version: 1,
        status: p::TextStatus::Available as i32,
        content: Some("A😀e\u{301}אב".into()),
        selection: Some(p::TextRange {
            anchor: 1,
            focus: 4,
        }),
        editable: true,
        selectable: true,
    };
    let binding = p::Binding {
        id: 41,
        version: 1,
        surface: Some(reference(p::ObjectKind::Surface, 10)),
        semantic_nodes: vec![reference(p::ObjectKind::Semantic, 31)],
        scene_nodes: vec![reference(p::ObjectKind::Scene, 21)],
        interaction_region: Some(rect()),
    };
    let session = p::SessionState {
        id: 50,
        version: 1,
        canvas: Some(rect()),
        focus: Some(reference(p::ObjectKind::Semantic, 31)),
        focus_surface: Some(reference(p::ObjectKind::Surface, 10)),
        focus_generation: 1,
        ..Default::default()
    };
    vec![
        wrap(p::record::Record::Surface(surface)),
        wrap(p::record::Record::Scene(root_scene)),
        wrap(p::record::Record::Scene(leaf_scene)),
        wrap(p::record::Record::Semantic(root)),
        wrap(p::record::Record::Semantic(button)),
        wrap(p::record::Record::Semantic(entry)),
        wrap(p::record::Record::Text(text)),
        wrap(p::record::Record::Binding(binding)),
        wrap(p::record::Record::Session(session)),
    ]
}
fn wire_semantic(records: &[p::Record], id: u64) -> p::SemanticNode {
    records
        .iter()
        .find_map(|r| match &r.record {
            Some(p::record::Record::Semantic(v)) if v.id == id => Some(v.clone()),
            _ => None,
        })
        .unwrap()
}
fn wire_scene(records: &[p::Record], id: u64) -> p::SceneNode {
    records
        .iter()
        .find_map(|r| match &r.record {
            Some(p::record::Record::Scene(v)) if v.id == id => Some(v.clone()),
            _ => None,
        })
        .unwrap()
}
fn wire_text(records: &[p::Record]) -> p::TextModel {
    records
        .iter()
        .find_map(|r| match &r.record {
            Some(p::record::Record::Text(v)) => Some(v.clone()),
            _ => None,
        })
        .unwrap()
}
fn wire_session(records: &[p::Record]) -> p::SessionState {
    records
        .iter()
        .find_map(|r| match &r.record {
            Some(p::record::Record::Session(v)) => Some(v.clone()),
            _ => None,
        })
        .unwrap()
}
fn loaded() -> Replica {
    let mut replica = Replica::new(Limits::default());
    replica
        .apply_snapshot(&snapshot(1, fixture()), &empty_resources())
        .unwrap();
    replica
}
fn unchanged_failure(replica: &mut Replica, bytes: &[u8]) -> ModelError {
    let before = replica.state().clone();
    let error = replica
        .apply_transaction(bytes, &empty_resources())
        .unwrap_err();
    assert_eq!(replica.state(), &before);
    error
}

#[test]
fn mod_001_complete_typed_snapshot_is_atomic_and_dangling_snapshot_leaves_previous_state() {
    let mut replica = loaded();
    assert_eq!(replica.state().revision, 1);
    assert_eq!(replica.state().records.len(), 9);
    let before = replica.state().clone();
    let mut records = fixture();
    records.retain(|r| !matches!(&r.record,Some(p::record::Record::Scene(v)) if v.id==21));
    assert!(replica
        .apply_snapshot(&snapshot(2, records), &empty_resources())
        .is_err());
    assert_eq!(replica.state(), &before);
}

#[test]
fn mod_002_reparent_in_child_first_wire_order_with_one_final_owner() {
    let mut replica = loaded();
    let records = fixture();
    let mut child = wire_semantic(&records, 31);
    child.version = 2;
    let mut root = wire_semantic(&records, 30);
    root.version = 2;
    root.ordered_children = vec![reference(p::ObjectKind::Semantic, 32)];
    let mut parent = wire_semantic(&records, 32);
    parent.version = 2;
    parent.ordered_children = vec![reference(p::ObjectKind::Semantic, 31)];
    replica
        .apply_transaction(
            &transaction(
                1,
                vec![
                    put(p::record::Record::Semantic(child)),
                    put(p::record::Record::Semantic(parent)),
                    put(p::record::Record::Semantic(root)),
                ],
            ),
            &empty_resources(),
        )
        .unwrap();
    let Record::Semantic(parent) = &replica.state().records[&32] else {
        panic!()
    };
    assert_eq!(parent.children[0].id, 31);
    let mut root = wire_semantic(&records, 30);
    root.version = 3;
    unchanged_failure(
        &mut replica,
        &transaction(2, vec![put(p::record::Record::Semantic(root))]),
    );
}

#[test]
fn mod_003_delete_requires_focus_binding_and_owner_repairs_together() {
    let mut replica = loaded();
    assert!(matches!(
        unchanged_failure(
            &mut replica,
            &transaction(1, vec![delete(p::ObjectKind::Semantic, 31)])
        ),
        ModelError::Reference(31)
    ));
    let records = fixture();
    let mut root = wire_semantic(&records, 30);
    root.version = 2;
    root.ordered_children.retain(|r| r.id != 31);
    let mut session = wire_session(&records);
    session.version = 2;
    session.focus = None;
    session.focus_surface = None;
    session.focus_generation = 2;
    replica
        .apply_transaction(
            &transaction(
                1,
                vec![
                    delete(p::ObjectKind::Semantic, 31),
                    delete(p::ObjectKind::Binding, 41),
                    put(p::record::Record::Session(session)),
                    put(p::record::Record::Semantic(root)),
                ],
            ),
            &empty_resources(),
        )
        .unwrap();
    assert!(!replica.state().records.contains_key(&31));
    assert!(replica.state().retired.contains(&31));
}

#[test]
fn mod_004_retirement_survives_snapshot_replacement_and_type_changes() {
    let mut replica = Replica::new(Limits::default());
    replica
        .apply_snapshot(
            &snapshot(1, vec![wrap(p::record::Record::Semantic(semantic(1, &[])))]),
            &empty_resources(),
        )
        .unwrap();
    replica
        .apply_transaction(
            &transaction(1, vec![delete(p::ObjectKind::Semantic, 1)]),
            &empty_resources(),
        )
        .unwrap();
    assert_eq!(
        unchanged_failure(
            &mut replica,
            &transaction(2, vec![put(p::record::Record::Scene(scene(1)))])
        ),
        ModelError::Retired(1)
    );
    replica
        .apply_snapshot(&snapshot(4, vec![]), &empty_resources())
        .unwrap();
    let before = replica.state().clone();
    assert!(matches!(
        replica.apply_snapshot(
            &snapshot(5, vec![wrap(p::record::Record::Semantic(semantic(1, &[])))]),
            &empty_resources()
        ),
        Err(ModelError::Retired(1))
    ));
    assert_eq!(replica.state(), &before);
}

#[test]
fn mod_005_ownership_cycles_fail_while_relation_cycles_are_valid() {
    let mut replica = loaded();
    let records = fixture();
    let mut a = wire_semantic(&records, 31);
    a.version = 2;
    a.relations = vec![p::Relation {
        kind: p::RelationKind::Controls as i32,
        targets: vec![reference(p::ObjectKind::Semantic, 32)],
    }];
    let mut b = wire_semantic(&records, 32);
    b.version = 2;
    b.relations = vec![p::Relation {
        kind: p::RelationKind::Controls as i32,
        targets: vec![reference(p::ObjectKind::Semantic, 31)],
    }];
    replica
        .apply_transaction(
            &transaction(
                1,
                vec![
                    put(p::record::Record::Semantic(a.clone())),
                    put(p::record::Record::Semantic(b)),
                ],
            ),
            &empty_resources(),
        )
        .unwrap();
    a.version = 3;
    a.ordered_children = vec![reference(p::ObjectKind::Semantic, 30)];
    assert!(matches!(
        unchanged_failure(
            &mut replica,
            &transaction(2, vec![put(p::record::Record::Semantic(a))])
        ),
        ModelError::Ownership(_)
    ));
}

#[test]
fn mod_006_full_replacement_clears_omitted_optionals() {
    let mut replica = loaded();
    let mut button = wire_semantic(&fixture(), 31);
    button.version = 2;
    button.description = None;
    button.typed_value = None;
    replica
        .apply_transaction(
            &transaction(1, vec![put(p::record::Record::Semantic(button))]),
            &empty_resources(),
        )
        .unwrap();
    let Record::Semantic(button) = &replica.state().records[&31] else {
        panic!()
    };
    assert_eq!(button.version, 2);
    assert_eq!(button.description, None);
    assert_eq!(button.value, None);
}

#[test]
fn mod_007_collection_selection_tracks_stable_id_during_reorder() {
    let mut root = semantic(30, &[31, 32]);
    root.role = p::SemanticRole::List as i32;
    root.collection_model = Some(p::CollectionModel {
        version: 1,
        items: root.ordered_children.clone(),
        total_count: Some(2),
        materialization: p::Materialization::FullyMaterialized as i32,
        selected_items: vec![reference(p::ObjectKind::Semantic, 31)],
    });
    let mut replica = Replica::new(Limits::default());
    replica
        .apply_snapshot(
            &snapshot(
                1,
                vec![
                    wrap(p::record::Record::Semantic(root.clone())),
                    wrap(p::record::Record::Semantic(semantic(31, &[]))),
                    wrap(p::record::Record::Semantic(semantic(32, &[]))),
                ],
            ),
            &empty_resources(),
        )
        .unwrap();
    root.version = 2;
    root.ordered_children.reverse();
    let collection = root.collection_model.as_mut().unwrap();
    collection.version = 2;
    collection.items.reverse();
    replica
        .apply_transaction(
            &transaction(1, vec![put(p::record::Record::Semantic(root))]),
            &empty_resources(),
        )
        .unwrap();
    let Record::Semantic(root) = &replica.state().records[&30] else {
        panic!()
    };
    let collection = root.collection.as_ref().unwrap();
    assert_eq!(collection.items[1].id, 31);
    assert_eq!(collection.selected_items[0].id, 31);
}

#[test]
fn mod_008_exact_versions_duplicate_mutations_and_type_replacements_reject() {
    let mut replica = loaded();
    for version in [1, 3, u64::MAX] {
        let mut node = wire_semantic(&fixture(), 31);
        node.version = version;
        assert_eq!(
            unchanged_failure(
                &mut replica,
                &transaction(1, vec![put(p::record::Record::Semantic(node))])
            ),
            ModelError::Version(31)
        );
    }
    let mut node = wire_semantic(&fixture(), 31);
    node.version = 2;
    let mutation = put(p::record::Record::Semantic(node));
    assert_eq!(
        unchanged_failure(
            &mut replica,
            &transaction(1, vec![mutation.clone(), mutation])
        ),
        ModelError::DuplicateId(31)
    );
    let mut replacement = scene(31);
    replacement.version = 2;
    assert_eq!(
        unchanged_failure(
            &mut replica,
            &transaction(1, vec![put(p::record::Record::Scene(replacement))])
        ),
        ModelError::Version(31)
    );
}

#[test]
fn mod_009_numeric_geometry_is_finite_bounded_premultiplied_and_normalized() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e10] {
        let mut replica = loaded();
        let mut node = wire_scene(&fixture(), 21);
        node.version = 2;
        node.transform.as_mut().unwrap().tx = value;
        assert!(matches!(
            unchanged_failure(
                &mut replica,
                &transaction(1, vec![put(p::record::Record::Scene(node))])
            ),
            ModelError::Invalid(_)
        ));
    }
    let mut replica = loaded();
    let mut node = wire_scene(&fixture(), 21);
    node.version = 2;
    node.transform.as_mut().unwrap().tx = -0.0;
    replica
        .apply_transaction(
            &transaction(1, vec![put(p::record::Record::Scene(node.clone()))]),
            &empty_resources(),
        )
        .unwrap();
    let Record::Scene(stored) = &replica.state().records[&21] else {
        panic!()
    };
    assert_eq!(stored.transform.tx.to_bits(), 0);
    node.version = 3;
    if let Some(p::scene_node::Primitive::SolidRect(solid)) = &mut node.primitive {
        solid.rect.as_mut().unwrap().width = -1.0;
    }
    unchanged_failure(
        &mut replica,
        &transaction(2, vec![put(p::record::Record::Scene(node.clone()))]),
    );
    if let Some(p::scene_node::Primitive::SolidRect(solid)) = &mut node.primitive {
        solid.rect = Some(rect());
        solid.color.as_mut().unwrap().alpha = 0.1;
    }
    unchanged_failure(
        &mut replica,
        &transaction(2, vec![put(p::record::Record::Scene(node))]),
    );
}

#[test]
fn mod_009_depth_and_record_limits_precede_publication() {
    let limits = Limits {
        max_depth: 2,
        ..Limits::default()
    };
    let mut replica = Replica::new(limits);
    let records = vec![
        wrap(p::record::Record::Semantic(semantic(1, &[2]))),
        wrap(p::record::Record::Semantic(semantic(2, &[]))),
    ];
    replica
        .apply_snapshot(&snapshot(1, records), &empty_resources())
        .unwrap();
    let mut parent = semantic(2, &[3]);
    parent.version = 2;
    assert_eq!(
        unchanged_failure(
            &mut replica,
            &transaction(
                1,
                vec![
                    put(p::record::Record::Semantic(parent)),
                    put(p::record::Record::Semantic(semantic(3, &[])))
                ]
            )
        ),
        ModelError::Limit("ownership depth")
    );
    let mut replica = Replica::new(Limits {
        max_records: 1,
        ..Limits::default()
    });
    assert!(matches!(
        replica.apply_snapshot(
            &snapshot(
                1,
                vec![
                    wrap(p::record::Record::Semantic(semantic(1, &[]))),
                    wrap(p::record::Record::Semantic(semantic(2, &[])))
                ]
            ),
            &empty_resources()
        ),
        Err(ModelError::Wire(WireError::Limit))
    ));
    assert_eq!(replica.state().revision, 0);
}

#[test]
fn snp_001_checkpoint_is_immutable_then_following_delta_applies() {
    let mut replica = Replica::new(Limits::default());
    let records = fixture();
    replica
        .apply_snapshot(&snapshot(10, records.clone()), &empty_resources())
        .unwrap();
    let Record::Semantic(button) = &replica.state().records[&31] else {
        panic!()
    };
    assert_eq!(button.label.as_deref(), Some("Save"));
    let mut button = wire_semantic(&records, 31);
    button.version = 2;
    button.label = Some("Saved".into());
    replica
        .apply_transaction(
            &transaction(10, vec![put(p::record::Record::Semantic(button))]),
            &empty_resources(),
        )
        .unwrap();
    assert_eq!(replica.state().revision, 11);
}

#[test]
fn txn_001_semantic_label_and_glyphs_commit_as_one_pair() {
    let mut replica = loaded();
    let records = fixture();
    let mut label = wire_semantic(&records, 31);
    label.version = 2;
    label.label = Some("Saved".into());
    let mut glyph = wire_scene(&records, 21);
    glyph.version = 2;
    glyph.primitive = Some(p::scene_node::Primitive::GlyphRun(p::GlyphRun {
        font: Some(p::glyph_run::Font::FontFingerprint(vec![7; 32])),
        size: 12.0,
        glyph_ids: "Saved".bytes().map(u32::from).collect(),
        positions: (0..5)
            .map(|i| p::Point {
                x: i as f64 * 7.0,
                y: 12.0,
            })
            .collect(),
        scalar_clusters: vec![0, 1, 2, 3, 4],
        color: Some(color()),
    }));
    let bytes = transaction(
        1,
        vec![
            put(p::record::Record::Semantic(label)),
            put(p::record::Record::Scene(glyph)),
        ],
    );
    let before = replica.state().clone();
    assert!(matches!(
        replica
            .apply_transaction(&bytes, &empty_resources())
            .unwrap(),
        ApplyOutcome::PendingResources { revision: 2, .. }
    ));
    assert_eq!(replica.state(), &before);
    replica.resume_pending(&BTreeSet::from([[7; 32]])).unwrap();
    let Record::Semantic(label) = &replica.state().records[&31] else {
        panic!()
    };
    assert_eq!(label.label.as_deref(), Some("Saved"));
    let Record::Scene(glyph) = &replica.state().records[&21] else {
        panic!()
    };
    assert!(
        matches!(&glyph.primitive,rui_protocol_core::model::Primitive::GlyphRun{glyph_ids,..} if glyph_ids.len()==5)
    );
}

#[test]
fn txn_002_and_004_stale_unknown_and_gap_require_recovery() {
    let mut replica = Replica::new(Limits::default());
    replica
        .apply_snapshot(&snapshot(10, fixture()), &empty_resources())
        .unwrap();
    for base in [8, 11, 20] {
        assert!(matches!(
            unchanged_failure(&mut replica, &transaction(base, vec![])),
            ModelError::Revision { current: 10 }
        ));
    }
    let overflow = p::Transaction {
        base_revision: u64::MAX,
        new_revision: 0,
        ..Default::default()
    }
    .encode_to_vec();
    unchanged_failure(&mut replica, &overflow);
}

#[test]
fn txn_003_exact_replay_acknowledges_without_versions_advancing_and_conflicts_fail() {
    let mut replica = loaded();
    let mut node = wire_semantic(&fixture(), 31);
    node.version = 2;
    let bytes = transaction(1, vec![put(p::record::Record::Semantic(node))]);
    replica
        .apply_transaction(&bytes, &empty_resources())
        .unwrap();
    let before = replica.state().clone();
    assert_eq!(
        replica
            .apply_transaction(&bytes, &empty_resources())
            .unwrap(),
        ApplyOutcome::Duplicate { revision: 2 }
    );
    assert_eq!(replica.state(), &before);
    let mut with_optional = bytes.clone();
    with_optional.extend_from_slice(&[0xa0, 0x06, 0x01]);
    assert_eq!(
        unchanged_failure(&mut replica, &with_optional),
        ModelError::DigestConflict
    );
}

#[test]
fn txn_005_applied_revisions_can_skip_presentation() {
    let mut replica = loaded();
    assert_eq!(replica.presented_revision(), None);
    for base in 1..4 {
        replica
            .apply_transaction(&transaction(base, vec![]), &empty_resources())
            .unwrap();
    }
    assert_eq!(replica.state().revision, 4);
    assert_eq!(replica.presented_revision(), None);
    replica.mark_presented(4).unwrap();
    assert_eq!(replica.presented_revision(), Some(4));
    assert_eq!(replica.mark_presented(3), Err(ModelError::Presentation));
    assert_eq!(replica.mark_presented(5), Err(ModelError::Presentation));
}

#[test]
fn neg_003_unknown_optional_fields_do_not_change_decoded_behavior() {
    let original = snapshot(1, vec![wrap(p::record::Record::Semantic(semantic(1, &[])))]);
    let mut extended = original.clone();
    extended.extend_from_slice(&[0xa0, 0x06, 0x01]);
    let mut a = Replica::new(Limits::default());
    let mut b = Replica::new(Limits::default());
    a.apply_snapshot(&original, &empty_resources()).unwrap();
    b.apply_snapshot(&extended, &empty_resources()).unwrap();
    assert_eq!(a.state(), b.state());
}

#[test]
fn neg_004_unknown_required_variants_reject_even_alongside_known_variants() {
    let mut replica = loaded();
    let message = p::Transaction {
        base_revision: 1,
        new_revision: 2,
        mutations: vec![p::Mutation { mutation: None }],
        ..Default::default()
    };
    let mut payload = message.encode_to_vec();
    assert!(payload.ends_with(&[0x1a, 0]));
    payload.truncate(payload.len() - 2);
    payload.extend_from_slice(&[0x1a, 2, 0x1a, 0]);
    assert!(matches!(
        unchanged_failure(&mut replica, &payload),
        ModelError::Wire(WireError::Unsupported(_))
    ));
    let mut node = wire_scene(&fixture(), 21);
    node.version = 2;
    node.primitive = None;
    unchanged_failure(
        &mut replica,
        &transaction(1, vec![put(p::record::Record::Scene(node))]),
    );
    // Scene group (tag 10) plus an unknown primitive (tag 14).
    let mut encoded = scene(99).encode_to_vec();
    encoded.extend_from_slice(&[0x72, 0]);
    let mut record = vec![0x12, encoded.len() as u8];
    record.extend(encoded);
    let mut mutation = vec![0x0a, record.len() as u8];
    mutation.extend(record);
    let mut txn = vec![8, 1, 16, 2, 26, mutation.len() as u8];
    txn.extend(mutation);
    assert!(matches!(
        unchanged_failure(&mut replica, &txn),
        ModelError::Wire(WireError::Unsupported(_))
    ));
}

#[test]
fn neg_005_envelope_payload_and_decoded_allocations_are_bounded_before_state_clone() {
    let limits = Limits {
        max_snapshot_bytes: 16,
        ..Limits::default()
    };
    let mut replica = Replica::new(limits);
    assert!(matches!(
        replica.apply_snapshot(&snapshot(1, fixture()), &empty_resources()),
        Err(ModelError::Wire(WireError::Limit))
    ));
    let budget = MemoryBudget::new(32_000);
    let mut replica = Replica::with_memory_budget(Limits::default(), budget.clone());
    replica
        .apply_snapshot(
            &snapshot(1, vec![wrap(p::record::Record::Semantic(semantic(1, &[])))]),
            &empty_resources(),
        )
        .unwrap();
    let used = budget.used();
    let hold = budget.reserve(budget.limit() - used - 10).unwrap();
    let before = replica.state().clone();
    assert!(matches!(
        replica.apply_transaction(&transaction(1, vec![]), &empty_resources()),
        Err(ModelError::Memory(_))
    ));
    assert_eq!(replica.state(), &before);
    drop(hold);
    assert_eq!(budget.used(), used);
    drop(replica);
    assert_eq!(budget.used(), 0);
}

#[test]
fn pending_closure_includes_cached_and_missing_resources_and_abort_releases_budget() {
    let budget = MemoryBudget::new(1 << 20);
    let mut replica = Replica::with_memory_budget(Limits::default(), budget.clone());
    let snapshot = p::Snapshot {
        revision: 1,
        records: vec![wrap(p::record::Record::Semantic(semantic(1, &[])))],
        required_resources: vec![vec![1; 32], vec![2; 32]],
        ..Default::default()
    }
    .encode_to_vec();
    let cached = BTreeSet::from([[1; 32]]);
    let outcome = replica.apply_snapshot(&snapshot, &cached).unwrap();
    let ApplyOutcome::PendingResources { revision, missing } = outcome else {
        panic!()
    };
    assert_eq!(revision, 1);
    assert_eq!(missing, BTreeSet::from([[2; 32]]));
    drop(missing);
    assert_eq!(
        replica.pending_resources(),
        Some(&BTreeSet::from([[1; 32], [2; 32]]))
    );
    assert!(budget.used() > 0);
    assert_eq!(replica.state().revision, 0);
    assert!(replica.abort_pending());
    assert_eq!(budget.used(), 0);
    replica
        .apply_snapshot(&snapshot, &BTreeSet::from([[1; 32], [2; 32]]))
        .unwrap();
    assert_eq!(replica.resource_refs(), &BTreeSet::from([[1; 32], [2; 32]]));
}

#[test]
fn pending_retransmits_are_exact_and_new_publications_cannot_overtake() {
    let mut replica = loaded();
    let before = replica.state().clone();
    let bytes = p::Transaction {
        base_revision: 1,
        new_revision: 2,
        required_resources: vec![vec![3; 32]],
        ..Default::default()
    }
    .encode_to_vec();
    replica
        .apply_transaction(&bytes, &empty_resources())
        .unwrap();
    assert!(matches!(
        replica.apply_transaction(&bytes, &empty_resources()),
        Ok(ApplyOutcome::PendingResources { revision: 2, .. })
    ));
    assert_eq!(
        unchanged_failure(&mut replica, &transaction(2, vec![])),
        ModelError::Pending
    );
    assert_eq!(
        unchanged_failure(&mut replica, &transaction(1, vec![])),
        ModelError::DigestConflict
    );
    assert_eq!(replica.state(), &before);
    replica.resume_pending(&BTreeSet::from([[3; 32]])).unwrap();
    assert_eq!(replica.state().revision, 2);
}

#[test]
fn text_scalar_selection_and_content_clock_enforce_preconditions() {
    let mut replica = loaded();
    let original = wire_text(&fixture());
    assert_eq!(original.content.as_ref().unwrap().chars().count(), 6);
    let mut replacement = original.clone();
    replacement.version = 2;
    replacement.content = Some("different".into());
    assert_eq!(
        unchanged_failure(
            &mut replica,
            &transaction(1, vec![put(p::record::Record::Text(replacement.clone()))])
        ),
        ModelError::Version(40)
    );
    replacement.text_version = 2;
    replica
        .apply_transaction(
            &transaction(1, vec![put(p::record::Record::Text(replacement.clone()))]),
            &empty_resources(),
        )
        .unwrap();
    replacement.version = 3;
    replacement.selection = Some(p::TextRange {
        anchor: 2,
        focus: 3,
    });
    replica
        .apply_transaction(
            &transaction(2, vec![put(p::record::Record::Text(replacement.clone()))]),
            &empty_resources(),
        )
        .unwrap();
    replacement.version = 4;
    replacement.selection = Some(p::TextRange {
        anchor: 0,
        focus: 100,
    });
    unchanged_failure(
        &mut replica,
        &transaction(3, vec![put(p::record::Record::Text(replacement))]),
    );
}

#[test]
fn redacted_text_cannot_smuggle_content_and_snapshot_clocks_cannot_regress() {
    let mut records = fixture();
    let text = records
        .iter_mut()
        .find_map(|r| match &mut r.record {
            Some(p::record::Record::Text(v)) => Some(v),
            _ => None,
        })
        .unwrap();
    text.status = p::TextStatus::TextRedacted as i32;
    let mut replica = Replica::new(Limits::default());
    assert!(replica
        .apply_snapshot(&snapshot(1, records), &empty_resources())
        .is_err());
    let mut replica = loaded();
    let mut records = fixture();
    let text = records
        .iter_mut()
        .find_map(|r| match &mut r.record {
            Some(p::record::Record::Text(v)) => Some(v),
            _ => None,
        })
        .unwrap();
    text.version = 2;
    text.content = Some("changed".into());
    let before = replica.state().clone();
    assert_eq!(
        replica.apply_snapshot(&snapshot(3, records), &empty_resources()),
        Err(ModelError::Version(40))
    );
    assert_eq!(replica.state(), &before);
}

#[test]
fn bindings_and_focus_cannot_cross_surface_ownership() {
    let mut records = fixture();
    let mut other_scene = scene(22);
    other_scene.primitive = Some(p::scene_node::Primitive::Group(p::Group {}));
    let mut surface = match records[0].record.clone().unwrap() {
        p::record::Record::Surface(v) => v,
        _ => panic!(),
    };
    surface.id = 11;
    surface.scene_root = Some(reference(p::ObjectKind::Scene, 22));
    surface.semantic_root = Some(reference(p::ObjectKind::Semantic, 33));
    records.extend([
        wrap(p::record::Record::Surface(surface)),
        wrap(p::record::Record::Scene(other_scene)),
        wrap(p::record::Record::Semantic(semantic(33, &[]))),
    ]);
    let mut replica = Replica::new(Limits::default());
    replica
        .apply_snapshot(&snapshot(1, records.clone()), &empty_resources())
        .unwrap();
    let mut binding = records
        .iter()
        .find_map(|r| match &r.record {
            Some(p::record::Record::Binding(v)) => Some(v.clone()),
            _ => None,
        })
        .unwrap();
    binding.version = 2;
    binding.scene_nodes = vec![reference(p::ObjectKind::Scene, 22)];
    assert!(matches!(
        unchanged_failure(
            &mut replica,
            &transaction(1, vec![put(p::record::Record::Binding(binding))])
        ),
        ModelError::Invalid(_)
    ));
    let mut session = wire_session(&records);
    session.version = 2;
    session.focus_surface = Some(reference(p::ObjectKind::Surface, 11));
    session.focus_generation = 2;
    assert!(matches!(
        unchanged_failure(
            &mut replica,
            &transaction(1, vec![put(p::record::Record::Session(session))])
        ),
        ModelError::Invalid(_)
    ));
}

#[test]
fn historical_digests_and_retired_id_metadata_are_bounded() {
    let mut replica = Replica::new(Limits {
        max_history: 1,
        max_retired: 1,
        ..Limits::default()
    });
    replica
        .apply_snapshot(
            &snapshot(
                1,
                vec![
                    wrap(p::record::Record::Semantic(semantic(1, &[]))),
                    wrap(p::record::Record::Semantic(semantic(2, &[]))),
                ],
            ),
            &empty_resources(),
        )
        .unwrap();
    let old = transaction(1, vec![delete(p::ObjectKind::Semantic, 1)]);
    replica.apply_transaction(&old, &empty_resources()).unwrap();
    replica
        .apply_transaction(&transaction(2, vec![]), &empty_resources())
        .unwrap();
    assert!(matches!(
        unchanged_failure(&mut replica, &old),
        ModelError::Revision { current: 3 }
    ));
    assert_eq!(
        unchanged_failure(
            &mut replica,
            &transaction(3, vec![delete(p::ObjectKind::Semantic, 2)])
        ),
        ModelError::Limit("retired IDs; start a new epoch")
    );
}

#[test]
fn envelopes_are_bound_to_negotiated_session_epoch_and_view() {
    let envelope = p::Envelope {
        protocol_version: 1,
        session_id: vec![1; 16],
        epoch: vec![2; 16],
        view_id: vec![3; 16],
        kind: p::MessageKind::Snapshot as i32,
        payload: snapshot(1, vec![]),
        required_features: vec!["content.no-raster.v1".into()],
    }
    .encode_to_vec();
    assert!(wire::decode_bound_envelope(
        &envelope,
        &Limits::default(),
        &[1; 16],
        &[2; 16],
        &[3; 16]
    )
    .is_ok());
    assert_eq!(
        wire::decode_bound_envelope(&envelope, &Limits::default(), &[1; 16], &[9; 16], &[3; 16]),
        Err(WireError::Envelope)
    );
    let mut unknown = p::Envelope::decode(envelope.as_slice()).unwrap();
    unknown.required_features.push("vendor.shader.v1".into());
    assert!(matches!(
        wire::decode_envelope(&unknown.encode_to_vec(), &Limits::default()),
        Err(WireError::Unsupported(_))
    ));
}

#[test]
fn malformed_protobuf_lengths_varints_and_message_kinds_fail_without_panics() {
    for bytes in [
        vec![0],
        vec![0x12, 0xff, 0xff, 0xff, 0xff, 0x7f],
        vec![
            0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02,
        ],
        vec![0x0b],
    ] {
        assert!(wire::decode_snapshot(&bytes, &Limits::default()).is_err());
    }
    let bytes = p::Envelope {
        protocol_version: 1,
        session_id: vec![1; 16],
        epoch: vec![2; 16],
        view_id: vec![3; 16],
        kind: 99,
        ..Default::default()
    }
    .encode_to_vec();
    assert_eq!(
        wire::decode_envelope(&bytes, &Limits::default()),
        Err(WireError::Envelope)
    );
}

#[test]
fn pending_request_outputs_are_charged_and_failed_history_growth_retains_candidate() {
    let budget = MemoryBudget::new(1 << 20);
    let mut replica = Replica::with_memory_budget(Limits::default(), budget.clone());
    let bytes = p::Snapshot {
        revision: 1,
        required_resources: vec![vec![7; 32]],
        ..Default::default()
    }
    .encode_to_vec();
    let output = replica.apply_snapshot(&bytes, &empty_resources()).unwrap();
    let with_output = budget.used();
    drop(output);
    let baseline = budget.used();
    assert_eq!(with_output - baseline, 608);
    let first = replica.resume_pending(&empty_resources()).unwrap();
    let second = replica.resume_pending(&empty_resources()).unwrap();
    assert_eq!(budget.used(), baseline + 1216);
    drop(first);
    drop(second);
    assert_eq!(budget.used(), baseline);
    let hold = budget.reserve(budget.limit() - baseline).unwrap();
    assert!(matches!(
        replica.resume_pending(&empty_resources()),
        Err(ModelError::Memory(_))
    ));
    assert!(matches!(
        replica.apply_snapshot(&bytes, &empty_resources()),
        Err(ModelError::Memory(_))
    ));
    assert!(matches!(
        replica.resume_pending(&BTreeSet::from([[7; 32]])),
        Err(ModelError::Memory(_))
    ));
    assert_eq!(replica.state().revision, 0);
    assert_eq!(
        replica.pending_resources(),
        Some(&BTreeSet::from([[7; 32]]))
    );
    drop(hold);
    assert_eq!(
        replica.resume_pending(&BTreeSet::from([[7; 32]])).unwrap(),
        ApplyOutcome::Applied { revision: 1 }
    );
    drop(replica);
    assert_eq!(budget.used(), 0);
}

#[test]
fn default_shared_budget_accepts_ten_thousand_records_and_a_following_replacement() {
    let limits = Limits::default();
    let budget = MemoryBudget::new(limits.max_memory_bytes);
    let mut replica = Replica::with_memory_budget(limits, budget.clone());
    let children = (2..=10_000).collect::<Vec<_>>();
    let mut records = Vec::with_capacity(10_000);
    records.push(wrap(p::record::Record::Semantic(semantic(1, &children))));
    for id in 2..=10_000 {
        records.push(wrap(p::record::Record::Semantic(semantic(id, &[]))));
    }
    replica
        .apply_snapshot(&snapshot(1, records), &empty_resources())
        .unwrap();
    let mut node = semantic(10_000, &[]);
    node.version = 2;
    node.label = Some("changed".into());
    replica
        .apply_transaction(
            &transaction(1, vec![put(p::record::Record::Semantic(node))]),
            &empty_resources(),
        )
        .unwrap();
    assert_eq!(replica.state().revision, 2);
    assert_eq!(replica.state().records.len(), 10_000);
    assert!(budget.used() <= budget.limit());
    drop(replica);
    assert_eq!(budget.used(), 0);
}

#[test]
fn repeated_singular_string_fields_keep_retained_capacity_charged_after_commit() {
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
    let mut node = semantic(1, &[]);
    node.label = None;
    let mut encoded = node.encode_to_vec();
    field(4, &vec![b'a'; 1 << 20], &mut encoded);
    field(4, b"x", &mut encoded);
    let mut record = Vec::new();
    field(3, &encoded, &mut record);
    let mut bytes = vec![8, 1];
    field(2, &record, &mut bytes);
    let decoded = wire::decode_snapshot(&bytes, &Limits::default()).unwrap();
    let Some(p::record::Record::Semantic(node)) = &decoded.records[0].record else {
        panic!()
    };
    assert_eq!(node.label.as_deref(), Some("x"));
    assert!(node.label.as_ref().unwrap().capacity() >= 1 << 20);
    drop(decoded);
    let budget = MemoryBudget::new(Limits::default().max_memory_bytes);
    let mut replica = Replica::with_memory_budget(Limits::default(), budget.clone());
    replica.apply_snapshot(&bytes, &empty_resources()).unwrap();
    let Record::Semantic(node) = &replica.state().records[&1] else {
        panic!()
    };
    assert_eq!(node.label.as_deref(), Some("x"));
    assert!(node.label.as_ref().unwrap().capacity() >= 1 << 20);
    assert!(budget.used() >= node.label.as_ref().unwrap().capacity());
    drop(replica);
    assert_eq!(budget.used(), 0);
}
