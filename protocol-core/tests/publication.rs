use rui_protocol_core::publication::*;
use rui_protocol_core::resources::*;

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

fn begin(id: u8, bytes: &[u8]) -> PublicationBegin {
    PublicationBegin {
        publication_id: [id; 16],
        kind: PublicationKind::Snapshot { revision: 10 },
        total_bytes: bytes.len() as u64,
        chunk_count: 2,
        item_count: 3,
        digest: publication_digest(bytes),
    }
}

fn assembler() -> (PublicationAssembler, MemoryBudget) {
    let budget = MemoryBudget::new(4 * 1024 * 1024);
    (
        PublicationAssembler::new(context(), PublicationLimits::default(), budget.clone()),
        budget,
    )
}

fn send(assembler: &mut PublicationAssembler, begin: &PublicationBegin, bytes: &[u8]) {
    assembler.begin(context(), begin.clone()).unwrap();
    assembler
        .push_chunk(context(), begin.publication_id, 0, &bytes[..2])
        .unwrap();
    assembler
        .push_chunk(context(), begin.publication_id, 1, &bytes[2..])
        .unwrap();
}

#[test]
fn snp_001_checkpoint_payload_is_exact_and_following_transaction_is_separate() {
    let checkpoint = b"revision ten bytes";
    let transaction = b"revision eleven bytes";
    let (mut assembler, _) = assembler();
    let snapshot = begin(1, checkpoint);
    let mut delta = begin(2, transaction);
    delta.kind = PublicationKind::Transaction {
        base_revision: 10,
        new_revision: 11,
    };
    assembler.begin(context(), snapshot.clone()).unwrap();
    assembler
        .push_chunk(context(), [1; 16], 0, &checkpoint[..2])
        .unwrap();
    send(&mut assembler, &delta, transaction);
    assembler
        .push_chunk(context(), [1; 16], 1, &checkpoint[2..])
        .unwrap();
    let checkpoint = assembler
        .finish(context(), [1; 16], PublicationCommit::from(&snapshot))
        .unwrap();
    assert_eq!(checkpoint.bytes(), b"revision ten bytes");
    assert_eq!(
        checkpoint.begin().kind,
        PublicationKind::Snapshot { revision: 10 }
    );
    let delta = assembler
        .finish(context(), [2; 16], PublicationCommit::from(&delta))
        .unwrap();
    assert_eq!(delta.bytes(), transaction);
    assert_eq!(
        delta.begin().kind,
        PublicationKind::Transaction {
            base_revision: 10,
            new_revision: 11
        }
    );
}

#[test]
fn snp_002_partial_replacement_has_no_output_and_abort_releases_memory() {
    let (mut assembler, budget) = assembler();
    let committed = b"revision ten remains".to_vec();
    let declared = begin(1, b"replacement checkpoint");
    assembler.begin(context(), declared.clone()).unwrap();
    assembler.push_chunk(context(), [1; 16], 0, b"re").unwrap();
    assert_eq!(assembler.staged_count(), 1);
    assert_eq!(assembler.staged_bytes(), 2);
    assert!(budget.used() > declared.total_bytes as usize);
    assembler.abort_all(context()).unwrap();
    assert_eq!(assembler.staged_count(), 0);
    assert_eq!(budget.used(), 0);
    assert_eq!(committed, b"revision ten remains");
}

#[test]
fn snp_003_commit_length_count_and_digest_metadata_must_match_begin() {
    let bytes = b"snapshot payload";
    for field in 0..4 {
        let (mut assembler, budget) = assembler();
        let declared = begin(1, bytes);
        send(&mut assembler, &declared, bytes);
        let mut commit = PublicationCommit::from(&declared);
        match field {
            0 => commit.total_bytes += 1,
            1 => commit.chunk_count += 1,
            2 => commit.item_count += 1,
            _ => commit.digest[0] ^= 1,
        }
        assert!(matches!(
            assembler.finish(context(), [1; 16], commit),
            Err(PublicationError::ConflictingCommit)
        ));
        assert_eq!(assembler.staged_count(), 0);
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn snp_003_missing_chunk_and_wrong_exact_body_digest_discard_staging() {
    let (mut assembler, budget) = assembler();
    let bytes = b"snapshot payload";
    let declared = begin(1, bytes);
    assembler.begin(context(), declared.clone()).unwrap();
    assembler
        .push_chunk(context(), [1; 16], 0, &bytes[..2])
        .unwrap();
    assert!(matches!(
        assembler.finish(context(), [1; 16], PublicationCommit::from(&declared)),
        Err(PublicationError::Incomplete)
    ));
    assert_eq!(budget.used(), 0);
    let changed = b"Snapshot payload";
    send(&mut assembler, &declared, changed);
    assert!(matches!(
        assembler.finish(context(), [1; 16], PublicationCommit::from(&declared)),
        Err(PublicationError::Integrity)
    ));
    assert_eq!(budget.used(), 0);
}

#[test]
fn snp_003_conflicting_duplicate_is_rejected_without_growing_or_overwriting() {
    let (mut assembler, budget) = assembler();
    let bytes = b"snapshot payload";
    let declared = begin(1, bytes);
    assembler.begin(context(), declared.clone()).unwrap();
    assert_eq!(
        assembler.begin(context(), declared.clone()),
        Ok(BeginOutcome::Duplicate)
    );
    assembler
        .push_chunk(context(), [1; 16], 0, &bytes[..2])
        .unwrap();
    let charge = budget.used();
    assert_eq!(
        assembler.push_chunk(context(), [1; 16], 0, &bytes[..2]),
        Ok(ChunkOutcome::Duplicate)
    );
    assert_eq!(
        assembler.push_chunk(context(), [1; 16], 0, b"xx"),
        Err(PublicationError::ConflictingDuplicate)
    );
    assert_eq!(assembler.staged_bytes(), 2);
    assert_eq!(budget.used(), charge);
    assert_eq!(
        assembler.push_chunk(context(), [1; 16], 1, &bytes[2..]),
        Err(PublicationError::ConflictingDuplicate)
    );
    assert!(matches!(
        assembler.finish(context(), [1; 16], PublicationCommit::from(&declared)),
        Err(PublicationError::ConflictingDuplicate)
    ));
    assert_eq!(assembler.staged_count(), 0);
    assert_eq!(budget.used(), 0);
    send(&mut assembler, &declared, bytes);
    assert_eq!(
        assembler
            .finish(context(), [1; 16], PublicationCommit::from(&declared))
            .unwrap()
            .bytes(),
        bytes
    );
}

#[test]
fn chunk_bounds_indices_empty_data_and_conflicting_begin_are_checked() {
    let (mut assembler, budget) = assembler();
    let bytes = b"snapshot payload";
    let declared = begin(1, bytes);
    assembler.begin(context(), declared.clone()).unwrap();
    let charge = budget.used();
    let mut conflict = declared.clone();
    conflict.item_count += 1;
    assert_eq!(
        assembler.begin(context(), conflict),
        Err(PublicationError::ConflictingBegin)
    );
    assert_eq!(
        assembler.push_chunk(context(), [1; 16], 1, b"skipped"),
        Err(PublicationError::NonContiguousChunk)
    );
    assert_eq!(
        assembler.push_chunk(context(), [1; 16], u32::MAX, b"skipped"),
        Err(PublicationError::NonContiguousChunk)
    );
    assert_eq!(
        assembler.push_chunk(context(), [1; 16], 0, b""),
        Err(PublicationError::Limit)
    );
    assert_eq!(
        assembler.push_chunk(context(), [1; 16], 0, &[0; 100]),
        Err(PublicationError::Limit)
    );
    assert_eq!(budget.used(), charge);
    assert_eq!(assembler.staged_bytes(), 0);
    assert_eq!(assembler.abort(context(), [1; 16]), Ok(true));
    assert_eq!(assembler.abort(context(), [1; 16]), Ok(false));
    assert_eq!(budget.used(), 0);
}

#[test]
fn neg_005_snapshot_transaction_counts_and_memory_are_bounded_before_growth() {
    let budget = MemoryBudget::new(4 * 1024 * 1024);
    let limits = PublicationLimits {
        max_snapshot_bytes: 100,
        max_transaction_bytes: 20,
        max_items: 3,
        ..PublicationLimits::default()
    };
    let mut assembler = PublicationAssembler::new(context(), limits, budget.clone());
    let base = begin(1, b"snapshot payload");
    let mut oversized = base.clone();
    oversized.total_bytes = u64::MAX;
    assert_eq!(
        assembler.begin(context(), oversized),
        Err(PublicationError::Limit)
    );
    let mut snapshot = base.clone();
    snapshot.total_bytes = 101;
    assert_eq!(
        assembler.begin(context(), snapshot),
        Err(PublicationError::Limit)
    );
    let mut counts = base.clone();
    counts.chunk_count = u32::MAX;
    assert_eq!(
        assembler.begin(context(), counts),
        Err(PublicationError::Limit)
    );
    let mut items = base.clone();
    items.item_count = u32::MAX;
    assert_eq!(
        assembler.begin(context(), items),
        Err(PublicationError::Limit)
    );
    let mut delta = begin(2, b"a transaction larger than limit");
    delta.kind = PublicationKind::Transaction {
        base_revision: 1,
        new_revision: 2,
    };
    assert_eq!(
        assembler.begin(context(), delta),
        Err(PublicationError::Limit)
    );
    let mut memory = begin(3, &[0; 80]);
    memory.chunk_count = 2;
    let held = budget.reserve(budget.limit() - budget.used()).unwrap();
    assert_eq!(
        assembler.begin(context(), memory),
        Err(PublicationError::Memory(MemoryError::Exceeded))
    );
    assert_eq!(assembler.staged_count(), 0);
    assert_eq!(budget.used(), budget.limit());
    drop(held);
    assert_eq!(budget.used(), 0);
}

#[test]
fn hostile_revision_overflow_and_inconsistent_chunk_claims_are_rejected() {
    let (mut assembler, _) = assembler();
    let bytes = b"snapshot payload";
    for kind in [
        PublicationKind::Snapshot { revision: 0 },
        PublicationKind::Transaction {
            base_revision: 0,
            new_revision: 1,
        },
        PublicationKind::Transaction {
            base_revision: 10,
            new_revision: 12,
        },
        PublicationKind::Transaction {
            base_revision: u64::MAX,
            new_revision: 0,
        },
    ] {
        let mut declared = begin(1, bytes);
        declared.kind = kind;
        assert_eq!(
            assembler.begin(context(), declared),
            Err(PublicationError::InvalidRevision)
        );
    }
    for (length, count) in [(0, 1), (10, 0), (1, 2), (128 * 1024, 1)] {
        let mut declared = begin(1, bytes);
        declared.total_bytes = length;
        declared.chunk_count = count;
        assert_eq!(
            assembler.begin(context(), declared),
            Err(PublicationError::Limit)
        );
    }
}

#[test]
fn assembled_bytes_remain_charged_until_reducer_owner_releases_output() {
    let (mut assembler, budget) = assembler();
    let bytes = b"snapshot payload";
    let declared = begin(1, bytes);
    send(&mut assembler, &declared, bytes);
    let output = assembler
        .finish(context(), [1; 16], PublicationCommit::from(&declared))
        .unwrap();
    assert_eq!(assembler.staged_count(), 0);
    assert!(budget.used() >= bytes.len());
    assert_eq!(output.bytes(), bytes);
    drop(assembler);
    assert!(budget.used() > 0);
    drop(output);
    assert_eq!(budget.used(), 0);
}

#[test]
fn auth_context_and_revocation_deny_chunks_and_finish_and_release_staging() {
    let (mut assembler, budget) = assembler();
    let bytes = b"snapshot payload";
    let declared = begin(1, bytes);
    for wrong in [
        ResourceContext {
            view_id: [5; 16],
            ..context()
        },
        ResourceContext {
            scope: ResourceScope {
                principal_id: [9; 16],
                ..context().scope
            },
            ..context()
        },
    ] {
        assert_eq!(
            assembler.begin(wrong, declared.clone()),
            Err(PublicationError::Unauthorized)
        );
    }
    send(&mut assembler, &declared, bytes);
    assembler.revoke();
    assert_eq!(assembler.staged_count(), 0);
    assert_eq!(budget.used(), 0);
    assert_eq!(
        assembler.push_chunk(context(), [1; 16], 0, bytes),
        Err(PublicationError::Unauthorized)
    );
    assert!(matches!(
        assembler.finish(context(), [1; 16], PublicationCommit::from(&declared)),
        Err(PublicationError::Unauthorized)
    ));
    assert_eq!(
        assembler.begin(context(), declared),
        Err(PublicationError::Unauthorized)
    );
}

#[test]
fn resources_and_publication_share_a_single_allocation_ceiling() {
    let budget = MemoryBudget::new(4 * 1024 * 1024);
    let mut cache = ResourceCache::new(context().scope, ResourceLimits::default(), budget.clone());
    cache.open_view(context().view_id).unwrap();
    let bytes = b"RVP1\0\0\0\0";
    let digest = resource_digest(
        ResourceKind::VectorPath,
        1,
        ResourceProvenance::AuthoredVector,
        bytes,
    );
    cache.authorize_resource(context().view_id, digest).unwrap();
    cache
        .receive_offer(
            context(),
            ResourceOffer {
                kind: ResourceKind::VectorPath,
                version: 1,
                total_bytes: 8,
                provenance: ResourceProvenance::AuthoredVector,
                digest,
            },
        )
        .unwrap();
    let before = budget.used();
    let held = budget.reserve(budget.limit() - before).unwrap();
    let mut assembler =
        PublicationAssembler::new(context(), PublicationLimits::default(), budget.clone());
    assert_eq!(
        assembler.begin(context(), begin(1, b"snapshot payload")),
        Err(PublicationError::Memory(MemoryError::Exceeded))
    );
    assert_eq!(budget.used(), budget.limit());
    cache.close_view(context().view_id);
    assert_eq!(budget.used(), held.bytes());
    drop(held);
    assert_eq!(budget.used(), 0);
    assembler
        .begin(context(), begin(1, b"snapshot payload"))
        .unwrap();
    assembler.abort_all(context()).unwrap();
    assert_eq!(budget.used(), 0);
}

#[test]
fn in_flight_count_limit_does_not_drop_an_accepted_checkpoint() {
    let budget = MemoryBudget::new(4 * 1024 * 1024);
    let limits = PublicationLimits {
        max_in_flight: 1,
        ..PublicationLimits::default()
    };
    let mut assembler = PublicationAssembler::new(context(), limits, budget.clone());
    let declared = begin(1, b"checkpoint payload");
    send(&mut assembler, &declared, b"checkpoint payload");
    let used = budget.used();
    assert_eq!(
        assembler.begin(context(), begin(2, b"second checkpoint")),
        Err(PublicationError::Limit)
    );
    assert_eq!(budget.used(), used);
    assert_eq!(
        assembler
            .finish(context(), [1; 16], PublicationCommit::from(&declared))
            .unwrap()
            .bytes(),
        b"checkpoint payload"
    );
}
