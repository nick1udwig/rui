use prost::Message;
use rui_protocol_core::publication::{
    publication_digest, BeginOutcome, PublicationBegin, PublicationCommit, PublicationError,
    PublicationKind,
};
use rui_protocol_core::receiver::{ReceiveOutcome, ReceiverConfig, ReceiverError, ViewReceiver};
use rui_protocol_core::resources::{
    resource_digest, ChunkOutcome, MemoryError, ResourceContext, ResourceError, ResourceKind,
    ResourceOffer, ResourceProvenance, ResourceScope,
};
use rui_protocol_core::session::ReceiverLimits;
use rui_protocol_core::wire::proto as pb;

fn context() -> ResourceContext {
    ResourceContext {
        scope: ResourceScope {
            session_id: [1; 16],
            epoch: [2; 16],
            principal_id: [3; 16],
        },
        view_id: [4; 16],
    }
}
fn receiver() -> ViewReceiver {
    ViewReceiver::new(
        context(),
        ReceiverLimits::default(),
        ReceiverConfig::default(),
    )
    .unwrap()
}
fn path() -> (ResourceOffer, Vec<u8>) {
    let mut bytes = b"RVP1".to_vec();
    bytes.extend_from_slice(&1u32.to_be_bytes());
    bytes.push(1);
    bytes.extend_from_slice(&0.0f64.to_be_bytes());
    bytes.extend_from_slice(&0.0f64.to_be_bytes());
    let digest = resource_digest(
        ResourceKind::VectorPath,
        1,
        ResourceProvenance::AuthoredVector,
        &bytes,
    );
    (
        ResourceOffer {
            kind: ResourceKind::VectorPath,
            version: 1,
            total_bytes: bytes.len() as u64,
            provenance: ResourceProvenance::AuthoredVector,
            digest,
        },
        bytes,
    )
}
fn scene(version: u64, resource: Option<[u8; 32]>) -> pb::Record {
    pb::Record {
        record: Some(pb::record::Record::Scene(pb::SceneNode {
            id: 1,
            version,
            opacity: 1.0,
            transform: Some(pb::Affine {
                xx: 1.0,
                yy: 1.0,
                ..Default::default()
            }),
            primitive: Some(match resource {
                Some(resource) => pb::scene_node::Primitive::FillPath(pb::FillPath {
                    resource: resource.to_vec(),
                    fill_rule: pb::FillRule::Nonzero as i32,
                    color: Some(pb::Color {
                        alpha: 1.0,
                        ..Default::default()
                    }),
                }),
                None => pb::scene_node::Primitive::Group(pb::Group {}),
            }),
            ..Default::default()
        })),
    }
}
fn snapshot(revision: u64, resource: Option<[u8; 32]>) -> Vec<u8> {
    pb::Snapshot {
        revision,
        records: vec![scene(revision, resource)],
        ..Default::default()
    }
    .encode_to_vec()
}
fn transaction(base: u64, resource: Option<[u8; 32]>) -> Vec<u8> {
    pb::Transaction {
        base_revision: base,
        new_revision: base + 1,
        mutations: vec![pb::Mutation {
            mutation: Some(pb::mutation::Mutation::Put(scene(base + 1, resource))),
        }],
        ..Default::default()
    }
    .encode_to_vec()
}
fn envelope(kind: pb::MessageKind, payload: Vec<u8>) -> Vec<u8> {
    let ctx = context();
    pb::Envelope {
        protocol_version: 1,
        session_id: ctx.scope.session_id.to_vec(),
        epoch: ctx.scope.epoch.to_vec(),
        view_id: ctx.view_id.to_vec(),
        kind: kind as i32,
        payload,
        ..Default::default()
    }
    .encode_to_vec()
}
fn submit(
    receiver: &mut ViewReceiver,
    id: u8,
    revision: u64,
    resource: Option<[u8; 32]>,
) -> Result<ReceiveOutcome, ReceiverError> {
    receiver.receive_envelope(
        [id; 16],
        &envelope(pb::MessageKind::Snapshot, snapshot(revision, resource)),
    )
}
fn deliver(
    receiver: &mut ViewReceiver,
    offer: &ResourceOffer,
    bytes: &[u8],
) -> Result<Option<ReceiveOutcome>, ReceiverError> {
    receiver.receive_resource_offer(context(), offer.clone())?;
    receiver.resource_chunk(context(), offer.digest, 0, bytes)?;
    receiver.complete_resource(context(), offer.digest)
}
fn begin(bytes: &[u8], id: u8, kind: PublicationKind, count: u32) -> PublicationBegin {
    PublicationBegin {
        publication_id: [id; 16],
        kind,
        total_bytes: bytes.len() as u64,
        chunk_count: 1,
        item_count: count,
        digest: publication_digest(bytes),
    }
}

#[test]
fn receiver_waits_owns_pins_and_applies_only_verified_resources() {
    let mut receiver = receiver();
    let (offer, bytes) = path();
    receiver.authorize_resource(offer.digest).unwrap();
    assert_eq!(
        receiver
            .receive_resource_offer(context(), offer.clone())
            .unwrap_err(),
        ReceiverError::NotNeeded
    );
    let pending = submit(&mut receiver, 5, 1, Some(offer.digest)).unwrap();
    assert!(
        matches!(&pending,ReceiveOutcome::NeedResources {revision:1,missing,..} if missing.contains(&offer.digest))
    );
    assert_eq!(receiver.state().unwrap().revision, 0);
    assert!(!receiver.resources_ready().unwrap());
    assert_eq!(
        receiver.mark_presented(1),
        Err(ReceiverError::NotPresentable)
    );
    assert_eq!(
        receiver.evict_resource(offer.digest),
        Err(ReceiverError::Resource(ResourceError::Pinned))
    );
    drop(pending);
    assert!(matches!(
        deliver(&mut receiver, &offer, &bytes).unwrap(),
        Some(ReceiveOutcome::Applied { revision: 1 })
    ));
    assert!(receiver.resources_ready().unwrap());
    assert_eq!(receiver.read_resource(offer.digest).unwrap(), bytes);
    assert_eq!(receiver.resource_offer(offer.digest).unwrap(), &offer);
    assert_eq!(receiver.presented_revision().unwrap(), None);
    assert_eq!(receiver.mark_presented(1).unwrap().revision, 1);
    assert_eq!(
        receiver.evict_resource(offer.digest),
        Err(ReceiverError::Resource(ResourceError::Pinned))
    );
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
}

#[test]
fn old_live_scene_and_pins_survive_pending_then_release_on_resource_free_swap() {
    let mut receiver = receiver();
    let (offer, bytes) = path();
    receiver.authorize_resource(offer.digest).unwrap();
    drop(submit(&mut receiver, 5, 1, Some(offer.digest)).unwrap());
    deliver(&mut receiver, &offer, &bytes).unwrap();
    let mut second_bytes = bytes.clone();
    let last = second_bytes.len() - 1;
    second_bytes[last] = 1;
    let second = ResourceOffer {
        digest: resource_digest(offer.kind, offer.version, offer.provenance, &second_bytes),
        ..offer.clone()
    };
    receiver.authorize_resource(second.digest).unwrap();
    let delta = envelope(
        pb::MessageKind::Transaction,
        transaction(1, Some(second.digest)),
    );
    assert!(matches!(
        receiver.receive_envelope([6; 16], &delta).unwrap(),
        ReceiveOutcome::NeedResources { revision: 2, .. }
    ));
    assert_eq!(receiver.state().unwrap().revision, 1);
    assert_eq!(receiver.mark_presented(1).unwrap().revision, 1);
    assert_eq!(
        receiver.evict_resource(offer.digest),
        Err(ReceiverError::Resource(ResourceError::Pinned))
    );
    receiver.abort_publication(context(), [6; 16]).unwrap();
    assert!(matches!(
        receiver
            .receive_envelope(
                [7; 16],
                &envelope(pb::MessageKind::Transaction, transaction(1, None))
            )
            .unwrap(),
        ReceiveOutcome::Applied { revision: 2 }
    ));
    assert_eq!(
        receiver.mark_presented(1),
        Err(ReceiverError::NotPresentable)
    );
    assert_eq!(
        receiver.evict_resource(offer.digest).unwrap().digest,
        offer.digest
    );
    assert_eq!(
        receiver
            .receive_resource_offer(context(), second)
            .unwrap_err(),
        ReceiverError::NotNeeded
    );
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
}

#[test]
fn failed_stage_pin_allocation_rolls_back_without_replacing_live_pins() {
    let mut config = ReceiverConfig::default();
    config.resources.max_pin_owners = 1;
    let mut receiver = ViewReceiver::new(context(), ReceiverLimits::default(), config).unwrap();
    let (offer, bytes) = path();
    receiver.authorize_resource(offer.digest).unwrap();
    drop(submit(&mut receiver, 5, 1, Some(offer.digest)).unwrap());
    deliver(&mut receiver, &offer, &bytes).unwrap();
    // The second staged owner would exceed the configured owner limit. It must
    // never replace the live owner before the atomic replica commit.
    assert_eq!(
        submit(&mut receiver, 6, 2, Some(offer.digest)).unwrap_err(),
        ReceiverError::Resource(ResourceError::Limit)
    );
    assert_eq!(receiver.state().unwrap().revision, 1);
    assert!(!receiver.has_pending_publication());
    assert_eq!(
        receiver.evict_resource(offer.digest),
        Err(ReceiverError::Resource(ResourceError::Pinned))
    );
    assert!(receiver.resources_ready().unwrap());
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
}

#[test]
fn conflicting_resource_duplicate_is_poisoned_until_safe_retry() {
    let mut receiver = receiver();
    let (offer, bytes) = path();
    receiver.authorize_resource(offer.digest).unwrap();
    drop(submit(&mut receiver, 5, 1, Some(offer.digest)).unwrap());
    receiver
        .receive_resource_offer(context(), offer.clone())
        .unwrap();
    assert_eq!(
        receiver
            .resource_chunk(context(), offer.digest, 1, &bytes)
            .unwrap_err(),
        ReceiverError::Resource(ResourceError::NonContiguousChunk)
    );
    receiver
        .resource_chunk(context(), offer.digest, 0, &bytes)
        .unwrap();
    assert_eq!(
        receiver
            .resource_chunk(context(), offer.digest, 0, &bytes)
            .unwrap(),
        ChunkOutcome::Duplicate
    );
    let mut changed = bytes.clone();
    changed[0] = 0;
    assert_eq!(
        receiver
            .resource_chunk(context(), offer.digest, 0, &changed)
            .unwrap_err(),
        ReceiverError::Resource(ResourceError::ConflictingDuplicate)
    );
    assert_eq!(
        receiver
            .complete_resource(context(), offer.digest)
            .unwrap_err(),
        ReceiverError::Resource(ResourceError::ConflictingDuplicate)
    );
    assert_eq!(receiver.state().unwrap().revision, 0);
    assert_eq!(receiver.cache_bytes().unwrap(), 0);
    assert!(matches!(
        deliver(&mut receiver, &offer, &bytes).unwrap(),
        Some(ReceiveOutcome::Applied { revision: 1 })
    ));
}

#[test]
fn wrong_resource_descriptor_is_rejected_before_allocating_a_body() {
    let mut receiver = receiver();
    let (offer, bytes) = path();
    let glyph = ResourceOffer {
        kind: ResourceKind::GlyphOutline,
        provenance: ResourceProvenance::FontOutline,
        digest: resource_digest(
            ResourceKind::GlyphOutline,
            1,
            ResourceProvenance::FontOutline,
            &bytes,
        ),
        ..offer
    };
    receiver.authorize_resource(glyph.digest).unwrap();
    drop(submit(&mut receiver, 5, 1, Some(glyph.digest)).unwrap());
    let before = receiver.memory_used();
    assert_eq!(
        receiver
            .receive_resource_offer(context(), glyph)
            .unwrap_err(),
        ReceiverError::ResourceKind
    );
    assert_eq!(receiver.memory_used(), before);
    assert_eq!(receiver.cache_bytes().unwrap(), 0);
    assert_eq!(receiver.state().unwrap().revision, 0);
}

#[test]
fn abort_prunes_grants_and_rejects_late_replies_without_memory_growth() {
    let mut config = ReceiverConfig::default();
    config.resources.max_authorizations = 1;
    let mut receiver = ViewReceiver::new(context(), ReceiverLimits::default(), config).unwrap();
    let baseline = receiver.memory_used();
    for index in 1u8..30 {
        let digest = [index; 32];
        receiver.authorize_resource(digest).unwrap();
        drop(submit(&mut receiver, index, 1, Some(digest)).unwrap());
        assert!(receiver.abort_publication(context(), [index; 16]).unwrap());
        assert_eq!(
            receiver.complete_resource(context(), digest).unwrap_err(),
            ReceiverError::NotNeeded
        );
        assert_eq!(
            receiver
                .resource_chunk(context(), digest, 0, b"late")
                .unwrap_err(),
            ReceiverError::NotNeeded
        );
        assert_eq!(receiver.memory_used(), baseline);
    }
}

#[test]
fn disconnect_discards_transfers_and_pending_state_preserves_live_then_reauthenticates() {
    let mut receiver = receiver();
    submit(&mut receiver, 5, 1, None).unwrap();
    receiver.mark_presented(1).unwrap();
    let live_memory = receiver.memory_used();
    let (offer, bytes) = path();
    receiver.authorize_resource(offer.digest).unwrap();
    drop(submit(&mut receiver, 6, 2, Some(offer.digest)).unwrap());
    receiver
        .receive_resource_offer(context(), offer.clone())
        .unwrap();
    receiver
        .resource_chunk(context(), offer.digest, 0, &bytes[..4])
        .unwrap();
    receiver.disconnect();
    assert_eq!(receiver.state().unwrap().revision, 1);
    assert_eq!(receiver.cache_bytes().unwrap(), 0);
    assert_eq!(receiver.presented_revision().unwrap(), None);
    assert_eq!(receiver.memory_used(), live_memory);
    assert_eq!(
        submit(&mut receiver, 7, 2, None).unwrap_err(),
        ReceiverError::Disconnected
    );
    let mut wrong = context();
    wrong.scope.epoch = [7; 16];
    assert_eq!(receiver.reconnect(wrong), Err(ReceiverError::Unauthorized));
    receiver.reconnect(context()).unwrap();
    assert!(receiver.resources_ready().unwrap());
    assert!(matches!(
        submit(&mut receiver, 7, 2, None).unwrap(),
        ReceiveOutcome::Applied { revision: 2 }
    ));
    receiver.revoke();
    assert_eq!(receiver.memory_used(), 0);
    assert_eq!(receiver.reconnect(context()), Err(ReceiverError::Closed));
}

#[test]
fn cache_loss_withholds_duplicate_and_presentation_until_reacquisition() {
    let mut receiver = receiver();
    let (offer, bytes) = path();
    receiver.authorize_resource(offer.digest).unwrap();
    drop(submit(&mut receiver, 5, 1, Some(offer.digest)).unwrap());
    deliver(&mut receiver, &offer, &bytes).unwrap();
    receiver.mark_presented(1).unwrap();
    receiver.resource_cache_lost().unwrap();
    assert_eq!(receiver.presented_revision().unwrap(), None);
    assert_eq!(
        receiver.mark_presented(1),
        Err(ReceiverError::NotPresentable)
    );
    assert_eq!(
        submit(&mut receiver, 6, 1, Some(offer.digest)).unwrap_err(),
        ReceiverError::NotPresentable
    );
    let missing = receiver.missing_live_resources().unwrap();
    assert!(missing.contains(&offer.digest));
    drop(missing);
    assert!(deliver(&mut receiver, &offer, &bytes).unwrap().is_none());
    assert!(receiver.resources_ready().unwrap());
    receiver.mark_presented(1).unwrap();
    assert!(matches!(
        submit(&mut receiver, 7, 1, Some(offer.digest)).unwrap(),
        ReceiveOutcome::Duplicate { revision: 1 }
    ));
}

#[test]
fn cache_loss_releases_unpinned_cache_grants_for_future_publications() {
    let mut config = ReceiverConfig::default();
    config.resources.max_authorizations = 1;
    let mut receiver = ViewReceiver::new(context(), ReceiverLimits::default(), config).unwrap();
    let (offer, bytes) = path();
    receiver.authorize_resource(offer.digest).unwrap();
    drop(submit(&mut receiver, 5, 1, Some(offer.digest)).unwrap());
    deliver(&mut receiver, &offer, &bytes).unwrap();
    submit(&mut receiver, 6, 2, None).unwrap();
    assert!(receiver.cache_bytes().unwrap() > 0);
    assert_eq!(
        receiver.authorize_resource([9; 32]),
        Err(ReceiverError::Resource(ResourceError::Limit))
    );
    receiver.resource_cache_lost().unwrap();
    assert_eq!(receiver.cache_bytes().unwrap(), 0);
    receiver.authorize_resource([9; 32]).unwrap();
    assert_eq!(receiver.state().unwrap().revision, 2);
}

#[test]
fn needed_resources_outputs_are_charged_bounded_and_survive_close_until_dropped() {
    let limits = ReceiverLimits {
        outbound_queue_bytes: 608,
        ..ReceiverLimits::default()
    };
    let mut receiver = ViewReceiver::new(context(), limits, ReceiverConfig::default()).unwrap();
    let (offer, _) = path();
    receiver.authorize_resource(offer.digest).unwrap();
    let held = submit(&mut receiver, 5, 1, Some(offer.digest)).unwrap();
    assert!(matches!(
        receiver.retry_pending(),
        Err(ReceiverError::Memory(MemoryError::Exceeded))
    ));
    assert_eq!(receiver.state().unwrap().revision, 0);
    receiver.close();
    assert_eq!(receiver.memory_used(), 608);
    drop(held);
    assert_eq!(receiver.memory_used(), 0);
}

#[test]
fn envelope_binding_and_resource_context_never_mutate_another_view() {
    let mut receiver = receiver();
    let mut forged =
        pb::Envelope::decode(envelope(pb::MessageKind::Snapshot, snapshot(1, None)).as_slice())
            .unwrap();
    forged.view_id = [9; 16].to_vec();
    assert!(receiver
        .receive_envelope([5; 16], &forged.encode_to_vec())
        .is_err());
    assert_eq!(receiver.state().unwrap().revision, 0);
    let bytes = snapshot(1, None);
    let start = begin(&bytes, 5, PublicationKind::Snapshot { revision: 1 }, 1);
    for mutate in 0..4 {
        let mut ctx = context();
        match mutate {
            0 => ctx.view_id = [9; 16],
            1 => ctx.scope.session_id = [9; 16],
            2 => ctx.scope.epoch = [9; 16],
            _ => ctx.scope.principal_id = [9; 16],
        }
        assert_eq!(
            receiver.begin_publication(ctx, start.clone()),
            Err(ReceiverError::Unauthorized)
        );
    }
    assert_eq!(
        receiver.begin_publication(
            context(),
            PublicationBegin {
                publication_id: [0; 16],
                ..start
            }
        ),
        Err(ReceiverError::InvalidId)
    );
    assert_eq!(
        receiver.authorize_resource([0; 32]),
        Err(ReceiverError::InvalidId)
    );
}

#[test]
fn assembler_metadata_single_flight_poison_abort_and_exact_replay_are_coordinated() {
    let mut receiver = receiver();
    let bytes = snapshot(1, None);
    let start = begin(&bytes, 5, PublicationKind::Snapshot { revision: 1 }, 1);
    assert_eq!(
        receiver
            .begin_publication(context(), start.clone())
            .unwrap(),
        BeginOutcome::Started
    );
    assert_eq!(
        receiver
            .begin_publication(context(), start.clone())
            .unwrap(),
        BeginOutcome::Duplicate
    );
    assert_eq!(
        receiver
            .begin_publication(
                context(),
                PublicationBegin {
                    publication_id: [6; 16],
                    ..start.clone()
                }
            )
            .unwrap_err(),
        ReceiverError::Busy
    );
    receiver
        .publication_chunk(context(), [5; 16], 0, &bytes)
        .unwrap();
    let mut changed = bytes.clone();
    changed.push(0);
    assert_eq!(
        receiver
            .publication_chunk(context(), [5; 16], 0, &changed)
            .unwrap_err(),
        ReceiverError::Publication(PublicationError::ConflictingDuplicate)
    );
    assert_eq!(
        receiver
            .commit_publication(context(), [5; 16], PublicationCommit::from(&start))
            .unwrap_err(),
        ReceiverError::Publication(PublicationError::ConflictingDuplicate)
    );
    assert!(!receiver.has_pending_publication());
    assert_eq!(receiver.state().unwrap().revision, 0);
    let lying = PublicationBegin {
        item_count: 0,
        ..start
    };
    receiver
        .begin_publication(context(), lying.clone())
        .unwrap();
    receiver
        .publication_chunk(context(), [5; 16], 0, &bytes)
        .unwrap();
    assert!(matches!(
        receiver.commit_publication(context(), [5; 16], PublicationCommit::from(&lying)),
        Err(ReceiverError::Model(_))
    ));
    assert!(!receiver.has_pending_publication());
    assert!(matches!(
        submit(&mut receiver, 7, 1, None).unwrap(),
        ReceiveOutcome::Applied { revision: 1 }
    ));
    assert!(matches!(
        submit(&mut receiver, 8, 1, None).unwrap(),
        ReceiveOutcome::Duplicate { revision: 1 }
    ));
    let mut changed = snapshot(1, None);
    changed.extend_from_slice(&[0x80, 0x08, 0x01]);
    assert!(matches!(
        receiver.receive_envelope([9; 16], &envelope(pb::MessageKind::Snapshot, changed)),
        Err(ReceiverError::Model(_))
    ));
    assert_eq!(receiver.state().unwrap().revision, 1);
    assert!(receiver
        .receive_envelope(
            [10; 16],
            &envelope(pb::MessageKind::Transaction, transaction(2, None))
        )
        .is_err());
    assert_eq!(receiver.state().unwrap().revision, 1);
}

#[test]
fn unauthorized_closure_and_unverified_fonts_roll_back_the_entire_candidate() {
    let mut receiver = receiver();
    let (offer, _) = path();
    assert_eq!(
        submit(&mut receiver, 5, 1, Some(offer.digest)).unwrap_err(),
        ReceiverError::Resource(ResourceError::NotAuthorized)
    );
    assert_eq!(receiver.state().unwrap().revision, 0);
    assert!(!receiver.has_pending_publication());
    let fingerprint = [8; 32];
    receiver.authorize_resource(fingerprint).unwrap();
    let mut record = scene(1, None);
    if let Some(pb::record::Record::Scene(scene)) = &mut record.record {
        scene.primitive = Some(pb::scene_node::Primitive::GlyphRun(pb::GlyphRun {
            font: Some(pb::glyph_run::Font::FontFingerprint(fingerprint.to_vec())),
            size: 12.0,
            glyph_ids: vec![1],
            positions: vec![pb::Point { x: 0.0, y: 0.0 }],
            scalar_clusters: vec![0],
            color: Some(pb::Color {
                alpha: 1.0,
                ..Default::default()
            }),
        }));
    }
    let bytes = pb::Snapshot {
        revision: 1,
        records: vec![record],
        ..Default::default()
    }
    .encode_to_vec();
    assert_eq!(
        receiver
            .receive_envelope([6; 16], &envelope(pb::MessageKind::Snapshot, bytes))
            .unwrap_err(),
        ReceiverError::UnsupportedFont
    );
    assert_eq!(receiver.state().unwrap().revision, 0);
    assert!(!receiver.has_pending_publication());
    receiver.authorize_resource(fingerprint).unwrap();
    let mut outline = scene(1, None);
    if let Some(pb::record::Record::Scene(scene)) = &mut outline.record {
        scene.primitive = Some(pb::scene_node::Primitive::GlyphRun(pb::GlyphRun {
            font: Some(pb::glyph_run::Font::OutlineResource(fingerprint.to_vec())),
            size: 12.0,
            glyph_ids: vec![41],
            positions: vec![pb::Point { x: 0.0, y: 0.0 }],
            scalar_clusters: vec![0],
            color: Some(pb::Color {
                alpha: 1.0,
                ..Default::default()
            }),
        }));
    }
    let bytes = pb::Snapshot {
        revision: 1,
        records: vec![outline],
        ..Default::default()
    }
    .encode_to_vec();
    assert_eq!(
        receiver
            .receive_envelope([7; 16], &envelope(pb::MessageKind::Snapshot, bytes))
            .unwrap_err(),
        ReceiverError::UnsupportedFont
    );
    assert_eq!(receiver.state().unwrap().revision, 0);
    assert!(!receiver.has_pending_publication());
    receiver.close();
    assert_eq!(receiver.memory_used(), 0);
}

#[test]
fn negotiated_record_count_and_ownership_depth_reject_candidates_without_state_change() {
    let records = ReceiverLimits {
        records: 1,
        ..ReceiverLimits::default()
    };
    let mut limited = ViewReceiver::new(context(), records, ReceiverConfig::default()).unwrap();
    submit(&mut limited, 5, 1, None).unwrap();
    let mut first = scene(2, None);
    let mut second = scene(1, None);
    if let Some(pb::record::Record::Scene(scene)) = &mut first.record {
        scene.ordered_children = vec![pb::ObjectRef {
            kind: pb::ObjectKind::Scene as i32,
            id: 2,
        }];
    }
    if let Some(pb::record::Record::Scene(scene)) = &mut second.record {
        scene.id = 2;
    }
    let bytes = pb::Snapshot {
        revision: 2,
        records: vec![first.clone(), second.clone()],
        ..Default::default()
    }
    .encode_to_vec();
    assert_eq!(
        limited
            .receive_envelope([6; 16], &envelope(pb::MessageKind::Snapshot, bytes.clone()))
            .unwrap_err(),
        ReceiverError::Publication(PublicationError::Limit)
    );
    assert_eq!(limited.state().unwrap().revision, 1);
    assert!(!limited.has_pending_publication());
    let depth = ReceiverLimits {
        ownership_depth: 1,
        ..ReceiverLimits::default()
    };
    let mut limited = ViewReceiver::new(context(), depth, ReceiverConfig::default()).unwrap();
    submit(&mut limited, 5, 1, None).unwrap();
    assert!(matches!(
        limited.receive_envelope([6; 16], &envelope(pb::MessageKind::Snapshot, bytes)),
        Err(ReceiverError::Model(_))
    ));
    assert_eq!(limited.state().unwrap().revision, 1);
    assert!(!limited.has_pending_publication());
}

#[test]
fn minimum_positive_chunks_handle_large_envelopes_with_a_bounded_chunk_count() {
    let limits = ReceiverLimits {
        chunk_bytes: 1,
        ..ReceiverLimits::default()
    };
    let mut config = ReceiverConfig::default();
    config.publication.max_chunks = 8192;
    let mut receiver = ViewReceiver::new(context(), limits.clone(), config).unwrap();
    let record = pb::Record {
        record: Some(pb::record::Record::Semantic(pb::SemanticNode {
            id: 1,
            version: 1,
            role: pb::SemanticRole::Root as i32,
            label: Some("a".repeat(5000)),
            sensitivity: pb::Sensitivity::Public as i32,
            completeness: pb::Completeness::Full as i32,
            ..Default::default()
        })),
    };
    let bytes = envelope(
        pb::MessageKind::Snapshot,
        pb::Snapshot {
            revision: 1,
            records: vec![record],
            ..Default::default()
        }
        .encode_to_vec(),
    );
    assert!(bytes.len() > 4096);
    assert!(matches!(
        receiver.receive_envelope([5; 16], &bytes).unwrap(),
        ReceiveOutcome::Applied { revision: 1 }
    ));
    let mut limited = ViewReceiver::new(context(), limits, ReceiverConfig::default()).unwrap();
    assert_eq!(
        limited.receive_envelope([5; 16], &bytes).unwrap_err(),
        ReceiverError::Publication(PublicationError::Limit)
    );
    assert_eq!(limited.state().unwrap().revision, 0);
    assert!(!limited.has_pending_publication());
}

#[test]
fn negotiated_ceilings_lower_envelopes_chunks_resources_items_depth_and_aggregate_memory() {
    let limits = ReceiverLimits {
        chunk_bytes: 8,
        resource_bytes: 16,
        ..ReceiverLimits::default()
    };
    let mut receiver = ViewReceiver::new(context(), limits, ReceiverConfig::default()).unwrap();
    let bytes = snapshot(1, None);
    let start = begin(&bytes, 5, PublicationKind::Snapshot { revision: 1 }, 1);
    // One declared chunk cannot hold this payload under the selected ceiling.
    assert_eq!(
        receiver.begin_publication(context(), start).unwrap_err(),
        ReceiverError::Publication(PublicationError::Limit)
    );
    assert!(matches!(
        submit(&mut receiver, 5, 1, None).unwrap(),
        ReceiveOutcome::Applied { revision: 1 }
    ));
    let (offer, _) = path();
    receiver.authorize_resource(offer.digest).unwrap();
    drop(submit(&mut receiver, 6, 2, Some(offer.digest)).unwrap());
    assert_eq!(
        receiver
            .receive_resource_offer(context(), offer)
            .unwrap_err(),
        ReceiverError::Resource(ResourceError::Limit)
    );
    let tiny = ReceiverLimits {
        envelope_bytes: 8,
        ..ReceiverLimits::default()
    };
    let mut bounded = ViewReceiver::new(context(), tiny, ReceiverConfig::default()).unwrap();
    assert!(submit(&mut bounded, 5, 1, None).is_err());
    let tiny = ReceiverLimits {
        view_memory_bytes: 6000,
        ..ReceiverLimits::default()
    };
    assert!(matches!(
        ViewReceiver::new(context(), tiny, ReceiverConfig::default()),
        Err(ReceiverError::Resource(ResourceError::Memory(
            MemoryError::Exceeded
        )))
    ));
}

#[test]
fn resource_revocation_disposes_live_scene_and_all_receiving_work() {
    let mut receiver = receiver();
    let (offer, bytes) = path();
    receiver.authorize_resource(offer.digest).unwrap();
    drop(submit(&mut receiver, 5, 1, Some(offer.digest)).unwrap());
    deliver(&mut receiver, &offer, &bytes).unwrap();
    receiver
        .revoke_resource_authorization(offer.digest)
        .unwrap();
    assert!(matches!(receiver.state(), Err(ReceiverError::Closed)));
    assert_eq!(receiver.memory_used(), 0);
    assert_eq!(
        receiver
            .complete_resource(context(), offer.digest)
            .unwrap_err(),
        ReceiverError::Closed
    );
}

#[test]
fn negotiated_single_outline_glyphs_require_verified_resources_and_zero_ids() {
    let (_, bytes) = path();
    let offer = ResourceOffer {
        kind: ResourceKind::GlyphOutline,
        version: 1,
        provenance: ResourceProvenance::FontOutline,
        total_bytes: bytes.len() as u64,
        digest: resource_digest(
            ResourceKind::GlyphOutline,
            1,
            ResourceProvenance::FontOutline,
            &bytes,
        ),
    };
    let mut node = scene(1, None);
    if let Some(pb::record::Record::Scene(scene)) = &mut node.record {
        scene.primitive = Some(pb::scene_node::Primitive::GlyphRun(pb::GlyphRun {
            font: Some(pb::glyph_run::Font::OutlineResource(offer.digest.to_vec())),
            size: 12.0,
            glyph_ids: vec![0, 0],
            positions: vec![pb::Point { x: 0.0, y: 12.0 }, pb::Point { x: 8.0, y: 12.0 }],
            scalar_clusters: vec![0, 1],
            color: Some(pb::Color {
                alpha: 1.0,
                ..Default::default()
            }),
        }));
    }
    let payload = |record: pb::Record| {
        envelope(
            pb::MessageKind::Snapshot,
            pb::Snapshot {
                revision: 1,
                records: vec![record],
                required_features: vec!["glyph.outline-single.v1".into()],
                ..Default::default()
            }
            .encode_to_vec(),
        )
    };
    let mut ordinary = receiver();
    ordinary.authorize_resource(offer.digest).unwrap();
    assert_eq!(
        ordinary
            .receive_envelope([5; 16], &payload(node.clone()))
            .unwrap_err(),
        ReceiverError::UnsupportedFont
    );
    let mut enabled = ViewReceiver::new(
        context(),
        ReceiverLimits::default(),
        ReceiverConfig {
            single_glyph_outlines: true,
            ..Default::default()
        },
    )
    .unwrap();
    enabled.authorize_resource(offer.digest).unwrap();
    assert!(matches!(
        enabled
            .receive_envelope([5; 16], &payload(node.clone()))
            .unwrap(),
        ReceiveOutcome::NeedResources { .. }
    ));
    assert_eq!(enabled.state().unwrap().revision, 0);
    assert!(matches!(
        deliver(&mut enabled, &offer, &bytes).unwrap(),
        Some(ReceiveOutcome::Applied { revision: 1 })
    ));
    enabled.mark_presented(1).unwrap();
    if let Some(pb::record::Record::Scene(scene)) = &mut node.record {
        if let Some(pb::scene_node::Primitive::GlyphRun(glyph)) = &mut scene.primitive {
            glyph.glyph_ids[0] = 1;
        }
    }
    let mut bad = ViewReceiver::new(
        context(),
        ReceiverLimits::default(),
        ReceiverConfig {
            single_glyph_outlines: true,
            ..Default::default()
        },
    )
    .unwrap();
    bad.authorize_resource(offer.digest).unwrap();
    assert_eq!(
        bad.receive_envelope([6; 16], &payload(node)).unwrap_err(),
        ReceiverError::UnsupportedFont
    );
    assert_eq!(bad.state().unwrap().revision, 0);
    assert!(!bad.has_pending_publication());
}
