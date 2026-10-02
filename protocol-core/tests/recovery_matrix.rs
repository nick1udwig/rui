//! Deterministic receiver faults at every publication/resource chunk boundary.
//! Presentation calls here are simulated protocol callbacks, not visual evidence.
use prost::Message;
use rui_protocol_core::commands::{
    AuthenticatedClient, CommandLedger, DispatchOutcome, LedgerLimits,
};
use rui_protocol_core::publication::{
    publication_digest, PublicationBegin, PublicationCommit, PublicationKind,
};
use rui_protocol_core::receiver::{ReceiveOutcome, ReceiverConfig, ViewReceiver};
use rui_protocol_core::resources::{
    resource_digest, ResourceContext, ResourceKind, ResourceOffer, ResourceProvenance,
    ResourceScope,
};
use rui_protocol_core::session::ReceiverLimits;
use rui_protocol_core::wire::proto as p;

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
        ReceiverLimits {
            view_memory_bytes: 2 << 20,
            ..Default::default()
        },
        ReceiverConfig::default(),
    )
    .unwrap()
}
fn vector(offset: f64) -> (ResourceOffer, Vec<u8>) {
    let mut bytes = b"RVP1".to_vec();
    bytes.extend(4u32.to_be_bytes());
    for (op, coords) in [
        (1, vec![offset, 0.0]),
        (2, vec![offset + 10.0, 0.0]),
        (2, vec![offset, 10.0]),
        (5, vec![]),
    ] {
        bytes.push(op);
        for n in coords {
            bytes.extend(n.to_be_bytes());
        }
    }
    let digest = resource_digest(
        ResourceKind::VectorPath,
        1,
        ResourceProvenance::AuthoredVector,
        &bytes,
    );
    (
        ResourceOffer {
            digest,
            kind: ResourceKind::VectorPath,
            version: 1,
            provenance: ResourceProvenance::AuthoredVector,
            total_bytes: bytes.len() as u64,
        },
        bytes,
    )
}
fn snapshot(revision: u64, scene: Option<(u64, [u8; 32])>) -> Vec<u8> {
    p::Snapshot {
        revision,
        records: scene
            .map(|(id, digest)| {
                vec![p::Record {
                    record: Some(p::record::Record::Scene(p::SceneNode {
                        id,
                        version: 1,
                        opacity: 1.0,
                        transform: Some(p::Affine {
                            xx: 1.0,
                            yy: 1.0,
                            ..Default::default()
                        }),
                        primitive: Some(p::scene_node::Primitive::FillPath(p::FillPath {
                            resource: digest.to_vec(),
                            fill_rule: p::FillRule::Nonzero as i32,
                            color: Some(p::Color {
                                alpha: 1.0,
                                ..Default::default()
                            }),
                        })),
                        ..Default::default()
                    })),
                }]
            })
            .unwrap_or_default(),
        ..Default::default()
    }
    .encode_to_vec()
}
fn envelope(payload: Vec<u8>) -> Vec<u8> {
    let c = context();
    p::Envelope {
        protocol_version: 1,
        session_id: c.scope.session_id.to_vec(),
        epoch: c.scope.epoch.to_vec(),
        view_id: c.view_id.to_vec(),
        kind: p::MessageKind::Snapshot as i32,
        payload,
        ..Default::default()
    }
    .encode_to_vec()
}
fn begin(payload: &[u8], id: u8, revision: u64, items: u32) -> PublicationBegin {
    PublicationBegin {
        publication_id: [id; 16],
        kind: PublicationKind::Snapshot { revision },
        total_bytes: payload.len() as u64,
        chunk_count: payload.len().div_ceil(17) as u32,
        item_count: items,
        digest: publication_digest(payload),
    }
}
fn deliver(receiver: &mut ViewReceiver, offer: &ResourceOffer, bytes: &[u8]) {
    receiver
        .receive_resource_offer(context(), offer.clone())
        .unwrap();
    for (i, chunk) in bytes.chunks(11).enumerate() {
        receiver
            .resource_chunk(context(), offer.digest, i as u32, chunk)
            .unwrap();
        receiver
            .resource_chunk(context(), offer.digest, i as u32, chunk)
            .unwrap();
    }
    receiver.complete_resource(context(), offer.digest).unwrap();
}

#[test]
fn pro_002_every_chunk_disconnect_retry_and_cache_loss_converges_without_redispatch() {
    let (offer, body) = vector(0.0);
    let candidate = snapshot(2, Some((2, offer.digest)));
    let exact = envelope(candidate.clone());
    let command = p::SemanticCommand {
        epoch: context().scope.epoch.to_vec(),
        view_id: context().view_id.to_vec(),
        client_instance_id: vec![5; 16],
        command_seq: 1,
        control_lease_id: vec![6; 16],
        control_generation: 1,
        target_id: 2,
        action_id: "Activate".into(),
        action_schema_version: 1,
        observed_view_revision: 1,
        arguments: Some(p::semantic_command::Arguments::NoArguments(
            p::EmptyArguments {},
        )),
        ..Default::default()
    }
    .encode_to_vec();
    let client = AuthenticatedClient {
        principal: "fault-fixture".into(),
        client_instance_id: [5; 16],
    };
    for publication_cut in 0..=candidate.len().div_ceil(17) {
        for resource_cut in 0..=body.len().div_ceil(11) {
            let mut ledger =
                CommandLedger::new(context().scope.epoch, LedgerLimits::default()).unwrap();
            let mut dispatches = 0;
            let mut dispatch = |_: &_| {
                dispatches += 1;
                Ok(DispatchOutcome::Dispatched)
            };
            let result = ledger
                .submit(&client, &command, 0, |_| Ok(()), &mut dispatch)
                .unwrap();
            let mut replica = receiver();
            replica
                .receive_envelope([7; 16], &envelope(snapshot(1, None)))
                .unwrap();
            let baseline = replica.state().unwrap().clone();
            let start = begin(&candidate, 8, 2, 1);
            replica.begin_publication(context(), start.clone()).unwrap();
            for (i, chunk) in candidate.chunks(17).take(publication_cut).enumerate() {
                replica
                    .publication_chunk(context(), start.publication_id, i as u32, chunk)
                    .unwrap();
                replica
                    .publication_chunk(context(), start.publication_id, i as u32, chunk)
                    .unwrap();
                assert_eq!(replica.state().unwrap(), &baseline);
            }
            replica.disconnect();
            assert!(!replica.has_pending_publication());
            assert_eq!(replica.state().unwrap(), &baseline);
            replica.reconnect(context()).unwrap();
            replica.authorize_resource(offer.digest).unwrap();
            let start = begin(&candidate, 9, 2, 1);
            replica.begin_publication(context(), start.clone()).unwrap();
            for (i, chunk) in candidate.chunks(17).enumerate() {
                replica
                    .publication_chunk(context(), start.publication_id, i as u32, chunk)
                    .unwrap();
            }
            assert!(matches!(
                replica
                    .commit_publication(
                        context(),
                        start.publication_id,
                        PublicationCommit::from(&start)
                    )
                    .unwrap(),
                ReceiveOutcome::NeedResources { revision: 2, .. }
            ));
            replica
                .receive_resource_offer(context(), offer.clone())
                .unwrap();
            for (i, chunk) in body.chunks(11).take(resource_cut).enumerate() {
                replica
                    .resource_chunk(context(), offer.digest, i as u32, chunk)
                    .unwrap();
            }
            assert!(replica.mark_presented(2).is_err());
            replica.disconnect();
            assert_eq!(replica.state().unwrap(), &baseline);
            replica.reconnect(context()).unwrap();
            replica.authorize_resource(offer.digest).unwrap();
            assert!(matches!(
                replica.receive_envelope([10; 16], &exact).unwrap(),
                ReceiveOutcome::NeedResources { revision: 2, .. }
            ));
            assert_eq!(
                ledger
                    .submit(
                        &client,
                        &command,
                        1,
                        |_| panic!("duplicate validation"),
                        |_| panic!("duplicate dispatch")
                    )
                    .unwrap(),
                result
            );
            deliver(&mut replica, &offer, &body);
            assert_eq!(replica.state().unwrap().revision, 2);
            assert!(matches!(
                replica.receive_envelope([11; 16], &exact).unwrap(),
                ReceiveOutcome::Duplicate { revision: 2 }
            ));
            replica.mark_presented(2).unwrap();
            replica.resource_cache_lost().unwrap();
            assert!(!replica.resources_ready().unwrap());
            assert!(replica.mark_presented(2).is_err());
            deliver(&mut replica, &offer, &body);
            assert!(replica.resources_ready().unwrap());
            let mut fresh = receiver();
            fresh.authorize_resource(offer.digest).unwrap();
            fresh.receive_envelope([12; 16], &exact).unwrap();
            deliver(&mut fresh, &offer, &body);
            assert_eq!(
                replica.state().unwrap(),
                fresh.state().unwrap(),
                "publication cut={publication_cut}, resource cut={resource_cut}"
            );
            assert_eq!(dispatches, 1);
            assert_eq!(ledger.high_water(&client), 1);
            assert!(replica.memory_used() <= replica.memory_limit());
            replica.close();
            fresh.close();
            assert_eq!(replica.memory_used(), 0);
            assert_eq!(fresh.memory_used(), 0);
        }
    }
}

#[test]
fn per_002_resource_churn_releases_bodies_and_preserves_bounded_retirement() {
    let mut replica = receiver();
    let mut previous = None;
    for revision in 1..=256u64 {
        let (offer, body) = vector(revision as f64);
        replica.authorize_resource(offer.digest).unwrap();
        replica
            .receive_envelope(
                (revision as u128).to_be_bytes(),
                &envelope(snapshot(revision, Some((revision, offer.digest)))),
            )
            .unwrap();
        deliver(&mut replica, &offer, &body);
        if let Some(digest) = previous {
            replica.evict_resource(digest).unwrap();
        }
        previous = Some(offer.digest);
        assert_eq!(replica.cache_bytes().unwrap(), body.len());
        assert_eq!(replica.state().unwrap().records.len(), 1);
        assert_eq!(
            replica.state().unwrap().retired.len(),
            revision as usize - 1
        );
        assert!(replica.memory_used() <= replica.memory_limit());
        if revision.is_multiple_of(8) {
            replica.disconnect();
            replica.reconnect(context()).unwrap();
        }
    }
    // Retirement is required epoch state, not a reclaimable cache. Ending the
    // view releases every body, pin, staged/history item and retirement record.
    replica.close();
    assert_eq!(replica.memory_used(), 0);
}
