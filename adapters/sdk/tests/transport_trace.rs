//! Exact post-auth TLS bytes and native source captures, with explicit callback scope.
#![cfg(feature = "transport")]
use prost::Message;
use rui_producer_sdk::CaptureArtifact;
use rui_protocol_core::{
    model::Record,
    publication::{PublicationBegin, PublicationCommit, PublicationKind},
    receiver::{ReceiveOutcome, ReceiverConfig, ViewReceiver},
    resources::{ResourceContext, ResourceOffer, ResourceScope},
    session::ReceiverLimits,
};
use rui_transport::proto as p;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
type ContextKey = ([u8; 16], [u8; 16], [u8; 16], [u8; 16]);

fn hex<const N: usize>(value: &Value) -> [u8; N] {
    let text = value.as_str().unwrap();
    assert_eq!(text.len(), 2 * N);
    std::array::from_fn(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap())
}
fn files(root: &Path, directory: &Path, names: &mut BTreeSet<String>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(root, &path, names);
        } else {
            assert!(!path.is_symlink());
            assert!(matches!(
                path.extension().unwrap().to_str().unwrap(),
                "json" | "bin" | "rvp"
            ));
            names.insert(path.strip_prefix(root).unwrap().to_str().unwrap().into());
        }
    }
}
fn modal(receiver: &ViewReceiver) -> bool {
    receiver.state().unwrap().records.values().any(|record| matches!(record, Record::Surface(surface) if surface.modal_for.is_some() && surface.mapped))
}
fn audit_applied(root: &Path, receiver: &ViewReceiver, outcome: ReceiveOutcome) -> Option<u64> {
    match outcome {
        ReceiveOutcome::Applied { revision } => {
            CaptureArtifact::read(root.join(format!("source-vector/snapshot-{revision:06}.json")))
                .unwrap()
                .verify_vector_audit(receiver)
                .unwrap();
            assert_eq!(modal(receiver), revision == 1);
            Some(revision)
        }
        ReceiveOutcome::NeedResources { .. } => None,
        ReceiveOutcome::Duplicate { .. } => panic!("unexpected duplicate source publication"),
    }
}

#[test]
fn actual_full_visual_tls_and_priority_native_lifecycle_traces_bind_exact_source_closure() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gtk");
    let report: Value =
        serde_json::from_slice(&fs::read(base.join("transport-interaction.json")).unwrap())
            .unwrap();
    assert_eq!(report["pinned_gtk"], serde_json::json!([4, 22, 4]));
    assert!(report["presentation_scope"]
        .as_str()
        .unwrap()
        .contains("simulated"));
    let root = base.join("transport-golden");
    let mut inventory = BTreeSet::new();
    files(&root, &root, &mut inventory);
    let hashes = report["retained_trace_sha256"].as_object().unwrap();
    assert_eq!(inventory, hashes.keys().cloned().collect());
    let mut retained_bytes = 0;
    for (name, hash) in hashes {
        let bytes = fs::read(root.join(name)).unwrap();
        retained_bytes += bytes.len();
        assert_eq!(
            format!("{:x}", Sha256::digest(bytes)),
            hash.as_str().unwrap()
        );
    }
    assert_eq!(report["retained_bytes"], retained_bytes);
    assert!(retained_bytes < 16 << 20);
    for scenario in ["full-visual", "disconnect", "revocation"] {
        let evidence = &report["scenarios"][scenario];
        assert_eq!(evidence["host_memory_after_disposal"], 0);
        assert!(evidence["host_reservation_peak_bytes"].as_u64().unwrap() <= 256 << 20);
        assert_eq!(evidence["negotiated_limits"]["view_memory_bytes"], 32 << 20);
        assert_eq!(evidence["negotiated_limits"]["envelope_bytes"], 128 << 10);
        assert_eq!(evidence["negotiated_limits"]["chunk_bytes"], 32 << 10);
        if scenario == "full-visual" {
            assert_eq!(evidence["exact_retry_equal"], true);
            assert_eq!(evidence["stalled_observer_then_applied_revision"], 2);
        } else {
            let queued = &evidence["queued_receipt"];
            let final_guard = &evidence["final_guard_receipt"];
            assert_eq!(queued["fence_generation"], 1);
            assert_eq!(final_guard["fence_generation"], 0);
            assert_eq!(final_guard["native_dispatches"], 0);
            assert_eq!(
                final_guard["private_request_id"],
                queued["private_request_id"]
            );
            assert!(
                final_guard["observed_monotonic_ns"].as_u64().unwrap()
                    > queued["observed_monotonic_ns"].as_u64().unwrap()
            );
            assert_eq!(
                evidence["approval_revoked_while_socket_alive"],
                scenario == "revocation"
            );
            assert_eq!(evidence["retained_failure_exact_retry"], "Failed");
            assert_eq!(evidence["fresh_scope_result_query_equal"], true);
            assert_eq!(evidence["fresh_generation"], 2);
        }
        let directory = root.join(scenario);
        let phases: &[&str] = if scenario == "full-visual" {
            &["full"]
        } else {
            &["before-loss", "after-reconnect"]
        };
        let mut receivers = BTreeMap::new();
        let mut applied = BTreeMap::<ContextKey, Vec<u64>>::new();
        let mut results = Vec::new();
        for phase in phases {
            let phase_root = directory.join("protocol").join(phase);
            let index: Value =
                serde_json::from_slice(&fs::read(phase_root.join("index.json")).unwrap()).unwrap();
            let mut sequences = BTreeMap::new();
            for entry in index.as_array().unwrap() {
                let context = ResourceContext {
                    scope: ResourceScope {
                        session_id: hex(&entry["session_id"]),
                        epoch: hex(&entry["epoch"]),
                        principal_id: hex(&entry["principal_id"]),
                    },
                    view_id: hex(&entry["view_id"]),
                };
                let key = (
                    context.scope.session_id,
                    context.scope.epoch,
                    context.scope.principal_id,
                    context.view_id,
                );
                let sequence = sequences.entry(key).or_insert(0);
                *sequence += 1;
                assert_eq!(entry["sequence"], *sequence);
                let bytes = fs::read(phase_root.join(entry["file"].as_str().unwrap())).unwrap();
                assert!(bytes.len() <= 128 << 10);
                assert_eq!(entry["wire_bytes"], bytes.len());
                assert_eq!(entry["sha256"], format!("{:x}", Sha256::digest(&bytes)));
                let packet = p::Packet::decode(bytes.as_slice()).unwrap();
                assert_eq!(packet.session_id, context.scope.session_id);
                assert_eq!(packet.epoch, context.scope.epoch);
                assert_eq!(packet.view_id, context.view_id);
                assert_eq!(packet.protocol_version, 1);
                let kind = p::Kind::try_from(packet.kind).unwrap();
                assert!(!matches!(kind, p::Kind::ClientHello | p::Kind::ServerHello));
                let receiver = receivers.entry(key).or_insert_with(|| {
                    ViewReceiver::new(
                        context,
                        ReceiverLimits {
                            envelope_bytes: 128 << 10,
                            chunk_bytes: 32 << 10,
                            ..Default::default()
                        },
                        ReceiverConfig {
                            single_glyph_outlines: true,
                            ..Default::default()
                        },
                    )
                    .unwrap()
                });
                if entry["direction"] != "HostToClient" {
                    assert_eq!(entry["direction"], "ClientToHost");
                    continue;
                }
                let outcome = match kind {
                    p::Kind::SnapshotBegin => {
                        let begin = p::PublicationBegin::decode(packet.payload.as_slice()).unwrap();
                        receiver
                            .begin_publication(
                                context,
                                PublicationBegin {
                                    publication_id: begin.publication_id.try_into().unwrap(),
                                    kind: PublicationKind::Snapshot {
                                        revision: begin.revision,
                                    },
                                    total_bytes: begin.total_bytes,
                                    chunk_count: begin.chunk_count,
                                    item_count: begin.item_count,
                                    digest: begin.digest.try_into().unwrap(),
                                },
                            )
                            .unwrap();
                        None
                    }
                    p::Kind::PublicationChunk => {
                        let chunk = p::PublicationChunk::decode(packet.payload.as_slice()).unwrap();
                        receiver
                            .publication_chunk(
                                context,
                                chunk.publication_id.try_into().unwrap(),
                                chunk.index,
                                &chunk.data,
                            )
                            .unwrap();
                        None
                    }
                    p::Kind::PublicationCommit => {
                        let commit =
                            p::PublicationCommit::decode(packet.payload.as_slice()).unwrap();
                        Some(
                            receiver
                                .commit_authenticated_publication(
                                    context,
                                    commit.publication_id.try_into().unwrap(),
                                    PublicationCommit {
                                        total_bytes: commit.total_bytes,
                                        chunk_count: commit.chunk_count,
                                        item_count: commit.item_count,
                                        digest: commit.digest.try_into().unwrap(),
                                    },
                                )
                                .unwrap(),
                        )
                    }
                    p::Kind::ResourceOffer => {
                        let offer = p::ResourceOffer::decode(packet.payload.as_slice()).unwrap();
                        receiver
                            .receive_resource_offer(
                                context,
                                ResourceOffer {
                                    digest: offer.digest.try_into().unwrap(),
                                    kind: offer.kind.try_into().unwrap(),
                                    version: offer.format_version,
                                    total_bytes: offer.total_bytes,
                                    provenance: offer.provenance.try_into().unwrap(),
                                },
                            )
                            .unwrap();
                        None
                    }
                    p::Kind::ResourceChunk => {
                        let chunk = p::ResourceChunk::decode(packet.payload.as_slice()).unwrap();
                        receiver
                            .resource_chunk(
                                context,
                                chunk.digest.try_into().unwrap(),
                                chunk.index,
                                &chunk.data,
                            )
                            .unwrap();
                        None
                    }
                    p::Kind::ResourceComplete => {
                        let complete =
                            p::ResourceComplete::decode(packet.payload.as_slice()).unwrap();
                        receiver
                            .complete_resource(context, complete.digest.try_into().unwrap())
                            .unwrap()
                    }
                    p::Kind::CommandResult => {
                        results.push(p::CommandResult::decode(packet.payload.as_slice()).unwrap());
                        None
                    }
                    _ => None,
                };
                if let Some(revision) =
                    outcome.and_then(|outcome| audit_applied(&directory, receiver, outcome))
                {
                    applied.entry(key).or_default().push(revision);
                }
            }
        }
        if scenario == "full-visual" {
            assert_eq!(applied.len(), 2);
            assert!(applied.values().all(|revisions| revisions == &[1, 2]));
            assert_eq!(results.len(), 2);
            assert_eq!(results[0], results[1]);
            assert_eq!(results[0].status, "Applied");
        } else {
            assert_eq!(applied.len(), 2);
            assert!(applied.values().any(|revisions| revisions == &[1]));
            assert!(applied.values().any(|revisions| revisions == &[1, 2]));
            assert!(results
                .iter()
                .any(|result| result.command_seq == 1 && result.status == "Failed"));
            assert!(results
                .iter()
                .any(|result| result.command_seq == 2 && result.status == "Applied"));
            let failed: Vec<_> = results
                .iter()
                .filter(|result| result.command_seq == 1 && result.status == "Failed")
                .collect();
            assert_eq!(failed.len(), 2);
            assert_eq!(failed[0], failed[1]);
        }
        let source =
            CaptureArtifact::read(directory.join("source-vector/snapshot-000002.json")).unwrap();
        for result in results.iter().filter(|result| result.status == "Applied") {
            assert_eq!(source.manifest()["capture_token"], result.source_commit);
        }
        assert_eq!(
            source.manifest()["command_cause"]["authority_gate"],
            "core-ledger-admitted"
        );
        for receiver in receivers.values_mut() {
            receiver.close();
            assert_eq!(receiver.memory_used(), 0);
        }
    }
}
