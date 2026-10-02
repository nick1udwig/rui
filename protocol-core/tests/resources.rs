use rui_protocol_core::resources::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

fn scope() -> ResourceScope {
    ResourceScope {
        session_id: [1; 16],
        epoch: [2; 16],
        principal_id: [3; 16],
    }
}

fn context(view: u8) -> ResourceContext {
    ResourceContext {
        scope: scope(),
        view_id: [view; 16],
    }
}

fn path(x: f64) -> Vec<u8> {
    let mut bytes = b"RVP1".to_vec();
    bytes.extend_from_slice(&3u32.to_be_bytes());
    for (opcode, coords) in [(1u8, vec![x, 0.0]), (2, vec![10.0, 20.0]), (5, vec![])] {
        bytes.push(opcode);
        for coord in coords {
            bytes.extend_from_slice(&coord.to_be_bytes());
        }
    }
    bytes
}

fn offer(bytes: &[u8]) -> ResourceOffer {
    ResourceOffer {
        kind: ResourceKind::VectorPath,
        version: 1,
        total_bytes: bytes.len() as u64,
        provenance: ResourceProvenance::AuthoredVector,
        digest: resource_digest(
            ResourceKind::VectorPath,
            1,
            ResourceProvenance::AuthoredVector,
            bytes,
        ),
    }
}

fn cache() -> (ResourceCache, MemoryBudget) {
    let budget = MemoryBudget::new(4 * 1024 * 1024);
    let mut cache = ResourceCache::new(scope(), ResourceLimits::default(), budget.clone());
    cache.open_view([4; 16]).unwrap();
    (cache, budget)
}

fn receive(cache: &mut ResourceCache, ctx: ResourceContext, bytes: &[u8]) -> ResourceDigest {
    let offer = offer(bytes);
    let digest = offer.digest;
    cache.authorize_resource(ctx.view_id, digest).unwrap();
    cache.receive_offer(ctx, offer).unwrap();
    cache.receive_chunk(ctx, digest, 0, bytes).unwrap();
    cache.complete(ctx, digest).unwrap();
    digest
}

#[test]
fn res_002_exact_integrity_and_res_006_fixed_domain_header() {
    let bytes = path(1.0);
    let declared = offer(&bytes);
    let mut reference = Sha256::new();
    reference.update(b"RUI/0.1/resource\0");
    reference.update(1u32.to_be_bytes());
    reference.update(1u32.to_be_bytes());
    reference.update((bytes.len() as u64).to_be_bytes());
    reference.update(1u32.to_be_bytes());
    reference.update(&bytes);
    let expected: [u8; 32] = reference.finalize().into();
    assert_eq!(declared.digest, expected);
    assert_ne!(
        declared.digest,
        resource_digest(
            ResourceKind::GlyphOutline,
            1,
            ResourceProvenance::FontOutline,
            &bytes
        )
    );
    assert_ne!(
        declared.digest,
        resource_digest(
            ResourceKind::VectorPath,
            2,
            ResourceProvenance::AuthoredVector,
            &bytes
        )
    );

    let (mut cache, budget) = cache();
    cache.authorize_resource([4; 16], declared.digest).unwrap();
    let baseline = budget.used();
    cache.receive_offer(context(4), declared.clone()).unwrap();
    assert_eq!(
        cache.read(context(4), declared.digest),
        Err(ResourceError::Unavailable)
    );
    let changed = path(2.0); // valid geometry, different exact bytes
    cache
        .receive_chunk(context(4), declared.digest, 0, &changed)
        .unwrap();
    assert_eq!(
        cache.complete(context(4), declared.digest),
        Err(ResourceError::Integrity)
    );
    assert_eq!(cache.cache_bytes(), 0);
    assert_eq!(budget.used(), baseline);
    assert!(cache.available_for_view(context(4)).unwrap().is_empty());
}

#[test]
fn res_003_valid_resource_is_shared_without_resending_body() {
    let (mut cache, _) = cache();
    let bytes = path(0.0);
    let digest = receive(&mut cache, context(4), &bytes);
    assert_eq!(cache.read(context(4), digest).unwrap(), bytes);
    assert_eq!(
        cache.receive_offer(context(4), offer(&bytes)),
        Ok(OfferOutcome::AlreadyAvailable)
    );
    cache.open_view([5; 16]).unwrap();
    assert_eq!(
        cache.read(context(5), digest),
        Err(ResourceError::NotAuthorized)
    );
    cache.authorize_resource([5; 16], digest).unwrap();
    assert_eq!(cache.read(context(5), digest).unwrap(), bytes);
    assert_eq!(cache.complete(context(5), digest).unwrap().contour_count, 1);
}

#[test]
fn res_001_and_res_004_missing_staged_live_and_inflight_owners_remain_pinned() {
    let (mut cache, _) = cache();
    let bytes = path(0.0);
    let digest = offer(&bytes).digest;
    cache.authorize_resource([4; 16], digest).unwrap();
    let required = BTreeSet::from([digest]);
    let staged = PinOwner::Staged {
        view_id: [4; 16],
        publication_id: [9; 16],
    };
    let live = PinOwner::Live { view_id: [4; 16] };
    let inflight = PinOwner::InFlight {
        view_id: [4; 16],
        publication_id: [8; 16],
    };
    cache.replace_pins(context(4), staged, &required).unwrap();
    assert!(cache.is_pinned(digest));
    receive(&mut cache, context(4), &bytes);
    assert_eq!(cache.evict(context(4), digest), Err(ResourceError::Pinned));
    cache.replace_pins(context(4), live, &required).unwrap();
    cache.replace_pins(context(4), inflight, &required).unwrap();
    cache.release_pins(context(4), staged).unwrap();
    cache.release_pins(context(4), live).unwrap();
    assert!(cache.evict_one_unpinned().is_none());
    cache.release_pins(context(4), inflight).unwrap();
    assert_eq!(cache.evict_one_unpinned(), Some(ResourceEvicted { digest }));
    assert_eq!(
        cache.read(context(4), digest),
        Err(ResourceError::Unavailable)
    );
}

#[test]
fn res_004_pressure_evicts_only_unpinned_and_returns_notification() {
    let (mut cache, _) = cache();
    let pinned = receive(&mut cache, context(4), &path(1.0));
    let disposable = receive(&mut cache, context(4), &path(2.0));
    cache
        .replace_pins(
            context(4),
            PinOwner::Live { view_id: [4; 16] },
            &BTreeSet::from([pinned]),
        )
        .unwrap();
    assert_eq!(
        cache.evict_one_unpinned(),
        Some(ResourceEvicted { digest: disposable })
    );
    assert!(cache.evict_one_unpinned().is_none());
    assert!(cache.read(context(4), pinned).is_ok());
}

#[test]
fn res_005_scope_view_and_revocation_are_checked_on_every_access() {
    let (mut cache, _) = cache();
    let bytes = path(1.0);
    let digest = receive(&mut cache, context(4), &bytes);
    for scope in [
        ResourceScope {
            principal_id: [8; 16],
            ..scope()
        },
        ResourceScope {
            session_id: [8; 16],
            ..scope()
        },
        ResourceScope {
            epoch: [8; 16],
            ..scope()
        },
    ] {
        let hostile = ResourceContext {
            scope,
            view_id: [4; 16],
        };
        assert_eq!(
            cache.read(hostile, digest),
            Err(ResourceError::Unauthorized)
        );
        assert_eq!(
            cache.receive_offer(hostile, offer(&bytes)),
            Err(ResourceError::Unauthorized)
        );
        assert_eq!(
            cache.complete(hostile, digest),
            Err(ResourceError::Unauthorized)
        );
    }
    assert_eq!(
        cache.read(context(8), digest),
        Err(ResourceError::UnknownView)
    );
    cache.revoke_resource([4; 16], digest);
    assert_eq!(
        cache.read(context(4), digest),
        Err(ResourceError::NotAuthorized)
    );
    assert_eq!(
        cache.complete(context(4), digest),
        Err(ResourceError::NotAuthorized)
    );
    assert_eq!(
        cache.receive_chunk(context(4), digest, 0, &bytes),
        Err(ResourceError::NotAuthorized)
    );
    assert_eq!(
        cache.replace_pins(
            context(4),
            PinOwner::Live { view_id: [4; 16] },
            &BTreeSet::from([digest])
        ),
        Err(ResourceError::NotAuthorized)
    );
    assert_eq!(cache.cache_bytes(), 0);
}

#[test]
fn chunk_duplicates_are_exact_bounded_contiguous_and_abortable() {
    let (mut cache, _) = cache();
    let bytes = path(0.0);
    let offer = offer(&bytes);
    let digest = offer.digest;
    cache.authorize_resource([4; 16], digest).unwrap();
    cache.receive_offer(context(4), offer).unwrap();
    assert_eq!(
        cache.receive_chunk(context(4), digest, u32::MAX, &bytes[..8]),
        Err(ResourceError::NonContiguousChunk)
    );
    assert_eq!(
        cache.receive_chunk(context(4), digest, 0, &[]),
        Err(ResourceError::Limit)
    );
    assert_eq!(
        cache.receive_chunk(context(4), digest, 0, &bytes[..8]),
        Ok(ChunkOutcome::Accepted)
    );
    assert_eq!(
        cache.receive_chunk(context(4), digest, 0, &bytes[..8]),
        Ok(ChunkOutcome::Duplicate)
    );
    assert_eq!(
        cache.receive_chunk(context(4), digest, 1, &vec![0; bytes.len()]),
        Err(ResourceError::Limit)
    );
    cache
        .receive_chunk(context(4), digest, 1, &bytes[8..])
        .unwrap();
    assert!(cache.complete(context(4), digest).is_ok());
    assert_eq!(cache.read(context(4), digest).unwrap(), bytes);
}

#[test]
fn conflicting_resource_duplicate_requires_fresh_transfer_without_clearing_pins() {
    let (mut cache, budget) = cache();
    let bytes = path(0.0);
    let declared = offer(&bytes);
    let digest = declared.digest;
    cache.authorize_resource([4; 16], digest).unwrap();
    cache
        .replace_pins(
            context(4),
            PinOwner::Staged {
                view_id: [4; 16],
                publication_id: [9; 16],
            },
            &BTreeSet::from([digest]),
        )
        .unwrap();
    let baseline = budget.used();
    cache.receive_offer(context(4), declared).unwrap();
    cache
        .receive_chunk(context(4), digest, 0, &bytes[..8])
        .unwrap();
    assert_eq!(
        cache.receive_chunk(context(4), digest, 0, b"conflict"),
        Err(ResourceError::ConflictingDuplicate)
    );
    assert_eq!(
        cache.receive_chunk(context(4), digest, 1, &bytes[8..]),
        Err(ResourceError::ConflictingDuplicate)
    );
    assert_eq!(
        cache.complete(context(4), digest),
        Err(ResourceError::ConflictingDuplicate)
    );
    assert_eq!(budget.used(), baseline);
    assert!(cache.is_pinned(digest));
    receive(&mut cache, context(4), &bytes);
    assert_eq!(cache.read(context(4), digest).unwrap(), bytes);
}

#[test]
fn incomplete_completion_frees_body_but_retains_missing_resource_pin() {
    let (mut cache, budget) = cache();
    let bytes = path(1.0);
    let offer = offer(&bytes);
    let digest = offer.digest;
    cache.authorize_resource([4; 16], digest).unwrap();
    cache
        .replace_pins(
            context(4),
            PinOwner::Staged {
                view_id: [4; 16],
                publication_id: [9; 16],
            },
            &BTreeSet::from([digest]),
        )
        .unwrap();
    let baseline = budget.used();
    cache.receive_offer(context(4), offer).unwrap();
    cache
        .receive_chunk(context(4), digest, 0, &bytes[..8])
        .unwrap();
    assert_eq!(
        cache.complete(context(4), digest),
        Err(ResourceError::Incomplete)
    );
    assert_eq!(budget.used(), baseline);
    assert!(cache.is_pinned(digest));
    receive(&mut cache, context(4), &bytes);
    assert_eq!(cache.evict(context(4), digest), Err(ResourceError::Pinned));
}

#[test]
fn neg_005_declared_resource_and_shared_memory_limits_precede_allocation() {
    let budget = MemoryBudget::new(4 * 1024 * 1024);
    let limits = ResourceLimits {
        max_resource_bytes: 1024,
        ..ResourceLimits::default()
    };
    let mut cache = ResourceCache::new(scope(), limits, budget.clone());
    cache.open_view([4; 16]).unwrap();
    let mut declared = offer(&path(0.0));
    cache.authorize_resource([4; 16], declared.digest).unwrap();
    let baseline = budget.used();
    declared.total_bytes = u64::MAX;
    assert_eq!(
        cache.receive_offer(context(4), declared.clone()),
        Err(ResourceError::Limit)
    );
    assert_eq!(budget.used(), baseline);
    declared.total_bytes = 1024;
    let held = budget.reserve(budget.limit() - budget.used()).unwrap();
    assert!(matches!(
        cache.receive_offer(context(4), declared),
        Err(ResourceError::Memory(MemoryError::Exceeded))
    ));
    assert_eq!(budget.used(), budget.limit());
    assert_eq!(cache.cache_bytes(), 0);
    drop(held);
    assert_eq!(budget.used(), baseline);
}

#[test]
fn no_raster_whitelist_and_provenance_are_fail_closed() {
    assert_eq!(
        ResourceKind::try_from(0),
        Err(ResourceError::UnsupportedKind)
    );
    assert_eq!(
        ResourceKind::try_from(99),
        Err(ResourceError::UnsupportedKind)
    );
    assert_eq!(
        ResourceProvenance::try_from(99),
        Err(ResourceError::UnsupportedProvenance)
    );
    let (mut cache, _) = cache();
    let mut declared = offer(&path(0.0));
    cache.authorize_resource([4; 16], declared.digest).unwrap();
    declared.provenance = ResourceProvenance::FontOutline;
    assert_eq!(
        cache.receive_offer(context(4), declared.clone()),
        Err(ResourceError::InvalidOffer)
    );
    declared.provenance = ResourceProvenance::AuthoredVector;
    declared.version = 2;
    assert_eq!(
        cache.receive_offer(context(4), declared),
        Err(ResourceError::InvalidOffer)
    );
}

#[test]
fn sec_003_and_sec_004_vector_decoder_rejects_active_and_malformed_bodies() {
    let mut malformed = vec![
        b"https://external.example/a".to_vec(),
        b"\x89PNG\r\n\x1a\n".to_vec(),
        b"shader arbitrary code".to_vec(),
        b"RVP1\0\0\0\x01\x02".to_vec(), // line before move
        b"RVP1\0\0\0\x01\x63".to_vec(), // unknown opcode
        b"RVP1\xff\xff\xff\xff".to_vec(),
    ];
    let mut trailing = path(0.0);
    trailing.push(0);
    malformed.push(trailing);
    malformed.extend([path(f64::NAN), path(f64::INFINITY), path(-0.0), path(1e10)]);
    let mut truncated = path(0.0);
    truncated.pop();
    malformed.push(truncated);
    for bytes in malformed {
        let (mut cache, _) = cache();
        let declared = offer(&bytes);
        cache.authorize_resource([4; 16], declared.digest).unwrap();
        cache.receive_offer(context(4), declared.clone()).unwrap();
        cache
            .receive_chunk(context(4), declared.digest, 0, &bytes)
            .unwrap();
        assert!(cache.complete(context(4), declared.digest).is_err());
        assert!(cache.available_for_view(context(4)).unwrap().is_empty());
        assert_eq!(cache.cache_bytes(), 0);
    }
}

#[test]
fn configured_invalid_coordinate_bound_cannot_bypass_validation() {
    for bound in [f64::NAN, f64::INFINITY, -1.0, 0.0] {
        let limits = ResourceLimits {
            max_coordinate_abs: bound,
            ..ResourceLimits::default()
        };
        assert_eq!(
            validate_vector_bytes(&path(0.0), &limits),
            Err(ResourceError::InvalidVector)
        );
    }
}

#[test]
fn closed_view_frees_unfinished_and_complete_unowned_bodies() {
    let (mut cache, budget) = cache();
    let complete = receive(&mut cache, context(4), &path(1.0));
    let declared = offer(&path(2.0));
    cache.authorize_resource([4; 16], declared.digest).unwrap();
    cache.receive_offer(context(4), declared.clone()).unwrap();
    cache
        .receive_chunk(context(4), declared.digest, 0, b"RVP1")
        .unwrap();
    cache
        .replace_pins(
            context(4),
            PinOwner::Live { view_id: [4; 16] },
            &BTreeSet::from([complete]),
        )
        .unwrap();
    assert!(budget.used() > 0);
    cache.close_view([4; 16]);
    assert_eq!(cache.cache_bytes(), 0);
    assert_eq!(budget.used(), 0);
    assert!(!cache.is_pinned(complete));
    assert_eq!(
        cache.complete(context(4), declared.digest),
        Err(ResourceError::UnknownView)
    );
}

#[test]
fn close_one_view_keeps_other_authorized_owner_and_revoke_purges_last() {
    let (mut cache, _) = cache();
    let bytes = path(1.0);
    let digest = receive(&mut cache, context(4), &bytes);
    cache.open_view([5; 16]).unwrap();
    cache.authorize_resource([5; 16], digest).unwrap();
    cache.close_view([4; 16]);
    assert_eq!(cache.read(context(5), digest).unwrap(), bytes);
    cache.revoke_resource([5; 16], digest);
    assert_eq!(cache.cache_bytes(), 0);
}

#[test]
fn revoked_body_is_retained_only_until_its_last_live_pin_is_released() {
    let (mut cache, _) = cache();
    let digest = receive(&mut cache, context(4), &path(0.0));
    let live = PinOwner::Live { view_id: [4; 16] };
    cache
        .replace_pins(context(4), live, &BTreeSet::from([digest]))
        .unwrap();
    cache.revoke_resource([4; 16], digest);
    assert_eq!(
        cache.read(context(4), digest),
        Err(ResourceError::NotAuthorized)
    );
    assert!(cache.is_pinned(digest));
    assert!(cache.cache_bytes() > 0);
    cache.release_pins(context(4), live).unwrap();
    assert_eq!(cache.cache_bytes(), 0);
}

#[test]
fn availability_output_and_failed_pin_replacement_respect_shared_budget() {
    let (mut cache, budget) = cache();
    let digest = receive(&mut cache, context(4), &path(0.0));
    let live = PinOwner::Live { view_id: [4; 16] };
    cache
        .replace_pins(context(4), live, &BTreeSet::from([digest]))
        .unwrap();
    let held = budget.reserve(budget.limit() - budget.used()).unwrap();
    assert!(matches!(
        cache.available_for_view(context(4)),
        Err(ResourceError::Memory(MemoryError::Exceeded))
    ));
    assert_eq!(
        cache.replace_pins(context(4), live, &BTreeSet::from([digest])),
        Err(ResourceError::Memory(MemoryError::Exceeded))
    );
    assert!(cache.is_pinned(digest));
    drop(held);
    // Four live digest keys fit in this allowance, but a fresh one-key
    // BTreeSet still needs a multi-slot root node. Per-key-only accounting
    // would incorrectly accept the availability allocation under pressure.
    let allowance = 4 * std::mem::size_of::<ResourceDigest>();
    let held = budget
        .reserve(budget.limit() - budget.used() - allowance)
        .unwrap();
    assert!(matches!(
        cache.available_for_view(context(4)),
        Err(ResourceError::Memory(MemoryError::Exceeded))
    ));
    drop(held);
    let before = budget.used();
    let available = cache.available_for_view(context(4)).unwrap();
    assert!(available.contains(&digest));
    let sparse_root_lower_bound =
        11 * std::mem::size_of::<ResourceDigest>() + 4 * std::mem::size_of::<usize>();
    assert!(budget.used() - before >= sparse_root_lower_bound);
    drop(available);
    assert_eq!(budget.used(), before);
}

#[test]
fn memory_reservations_are_shared_atomic_and_released_by_owner() {
    let budget = MemoryBudget::new(100);
    let mut first = budget.reserve(60).unwrap();
    let second = budget.clone().reserve(30).unwrap();
    assert_eq!(budget.used(), 90);
    assert_eq!(first.resize(80), Err(MemoryError::Exceeded));
    assert_eq!(first.bytes(), 60);
    first.resize(50).unwrap();
    assert_eq!(budget.used(), 80);
    drop(second);
    assert_eq!(budget.used(), 50);
    drop(first);
    assert_eq!(budget.used(), 0);
}
