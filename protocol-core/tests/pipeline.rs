//! Cross-module receiver tests: exact publication bytes, safe resources, atomic
//! state, and ownership accounting share one bounded allocation ledger.
use prost::Message;
use rui_protocol_core::model::{ApplyOutcome, Limits, Replica};
use rui_protocol_core::publication::{
    publication_digest, AssembledPublication, PublicationAssembler, PublicationBegin,
    PublicationCommit, PublicationKind, PublicationLimits,
};
use rui_protocol_core::resources::{
    resource_digest, MemoryBudget, PinOwner, ResourceCache, ResourceContext, ResourceError,
    ResourceKind, ResourceLimits, ResourceOffer, ResourceProvenance, ResourceScope,
};
use rui_protocol_core::wire::proto as pb;
use std::collections::BTreeSet;

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

fn vector() -> Vec<u8> {
    let mut data = b"RVP1".to_vec();
    data.extend_from_slice(&3u32.to_be_bytes());
    for (opcode, coordinates) in [(1u8, vec![0.0f64, 0.0]), (2, vec![10.0, 10.0]), (5, vec![])] {
        data.push(opcode);
        for coordinate in coordinates {
            data.extend_from_slice(&coordinate.to_be_bytes());
        }
    }
    data
}

fn snapshot(digest: [u8; 32]) -> Vec<u8> {
    pb::Snapshot {
        revision: 1,
        records: vec![pb::Record {
            record: Some(pb::record::Record::Scene(pb::SceneNode {
                id: 1,
                version: 1,
                opacity: 1.0,
                transform: Some(pb::Affine {
                    xx: 1.0,
                    yy: 1.0,
                    ..Default::default()
                }),
                primitive: Some(pb::scene_node::Primitive::FillPath(pb::FillPath {
                    resource: digest.to_vec(),
                    fill_rule: pb::FillRule::Nonzero as i32,
                    color: Some(pb::Color {
                        red: 1.0,
                        green: 0.0,
                        blue: 0.0,
                        alpha: 1.0,
                    }),
                })),
                resource_refs: vec![digest.to_vec()],
                ..Default::default()
            })),
        }],
        required_resources: vec![digest.to_vec()],
        required_features: vec![],
    }
    .encode_to_vec()
}

fn assemble(budget: MemoryBudget, bytes: &[u8], item_count: u32) -> AssembledPublication {
    let ctx = context();
    let mut assembler = PublicationAssembler::new(ctx, PublicationLimits::default(), budget);
    let begin = PublicationBegin {
        publication_id: [5; 16],
        kind: PublicationKind::Snapshot { revision: 1 },
        total_bytes: bytes.len() as u64,
        chunk_count: 2,
        item_count,
        digest: publication_digest(bytes),
    };
    assembler.begin(ctx, begin.clone()).unwrap();
    let middle = bytes.len() / 2;
    assembler
        .push_chunk(ctx, begin.publication_id, 0, &bytes[..middle])
        .unwrap();
    assembler
        .push_chunk(ctx, begin.publication_id, 1, &bytes[middle..])
        .unwrap();
    assembler
        .finish(ctx, begin.publication_id, PublicationCommit::from(&begin))
        .unwrap()
}

#[test]
fn missing_resource_waits_then_commits_with_pins_and_shared_budget() {
    let ctx = context();
    let budget = MemoryBudget::new(2 * 1024 * 1024);
    let path = vector();
    let digest = resource_digest(
        ResourceKind::VectorPath,
        1,
        ResourceProvenance::AuthoredVector,
        &path,
    );
    let required: BTreeSet<_> = [digest].into_iter().collect();
    let mut cache = ResourceCache::new(ctx.scope, ResourceLimits::default(), budget.clone());
    cache.open_view(ctx.view_id).unwrap();
    cache.authorize_resource(ctx.view_id, digest).unwrap();
    let mut replica = Replica::with_memory_budget(Limits::default(), budget.clone());
    let publication = assemble(budget.clone(), &snapshot(digest), 1);
    let staged = PinOwner::Staged {
        view_id: ctx.view_id,
        publication_id: [5; 16],
    };
    cache.replace_pins(ctx, staged, &required).unwrap();
    let available = cache.available_for_view(ctx).unwrap();
    assert!(matches!(
        replica.apply_publication(&publication, &available).unwrap(),
        ApplyOutcome::PendingResources { revision: 1, .. }
    ));
    drop(available);
    drop(publication);
    assert_eq!(replica.state().revision, 0);
    assert!(replica.mark_presented(1).is_err());
    cache
        .receive_offer(
            ctx,
            ResourceOffer {
                kind: ResourceKind::VectorPath,
                version: 1,
                total_bytes: path.len() as u64,
                provenance: ResourceProvenance::AuthoredVector,
                digest,
            },
        )
        .unwrap();
    cache.receive_chunk(ctx, digest, 0, &path).unwrap();
    cache.complete(ctx, digest).unwrap();
    assert_eq!(cache.evict(ctx, digest), Err(ResourceError::Pinned));
    let available = cache.available_for_view(ctx).unwrap();
    // Pin the future live closure before releasing the staged owner's pins.
    cache
        .replace_pins(
            ctx,
            PinOwner::Live {
                view_id: ctx.view_id,
            },
            &required,
        )
        .unwrap();
    assert!(matches!(
        replica.resume_pending(&available).unwrap(),
        ApplyOutcome::Applied { revision: 1 }
    ));
    drop(available);
    cache.release_pins(ctx, staged).unwrap();
    replica.mark_presented(1).unwrap();
    assert_eq!(replica.presented_revision(), Some(1));
    assert_eq!(cache.evict(ctx, digest), Err(ResourceError::Pinned));
    assert!(budget.used() <= budget.limit());
    drop(replica);
    cache.close_view(ctx.view_id);
    drop(cache);
    assert_eq!(
        budget.used(),
        0,
        "all owners release their allocation reservations"
    );
}

#[test]
fn verified_outer_digest_does_not_excuse_false_decoded_item_count() {
    let budget = MemoryBudget::new(2 * 1024 * 1024);
    let publication = assemble(budget.clone(), &snapshot([7; 32]), 0);
    let mut replica = Replica::with_memory_budget(Limits::default(), budget.clone());
    assert!(replica
        .apply_publication(&publication, &BTreeSet::new())
        .is_err());
    assert_eq!(replica.state().revision, 0);
    assert!(replica.pending_resources().is_none());
    drop(publication);
    drop(replica);
    assert_eq!(budget.used(), 0);
}
