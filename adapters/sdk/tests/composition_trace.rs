//! Frozen actual TLS/native text composition; renderer callbacks are simulated
//! in this producer probe, while the separate desktop probe owns GUI evidence.
#![cfg(feature = "transport")]
use data_encoding::HEXLOWER;
use prost::Message;
use rui_producer_sdk::CaptureArtifact;
use rui_protocol_core::{
    model::Record,
    publication::{PublicationBegin, PublicationCommit, PublicationKind},
    receiver::{ReceiveOutcome, ReceiverConfig, ViewReceiver},
    resources::{ResourceContext, ResourceOffer, ResourceScope},
    session::ReceiverLimits,
    wire::proto as core,
};
use rui_transport::proto as packet;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

const PREEDIT: &str = "私😀e\u{301}";
type Key = ([u8; 16], [u8; 16]);
fn inventory(root: &Path, directory: &Path, files: &mut BTreeSet<String>) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        assert!(!kind.is_symlink());
        if kind.is_dir() {
            inventory(root, &entry.path(), files);
        } else {
            assert!(kind.is_file());
            files.insert(
                entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned(),
            );
        }
    }
}
fn hex<const N: usize>(value: &Value) -> [u8; N] {
    HEXLOWER
        .decode(value.as_str().unwrap().as_bytes())
        .unwrap()
        .try_into()
        .unwrap()
}
fn read(path: &Path, ceiling: u64) -> Vec<u8> {
    assert!(fs::metadata(path).unwrap().len() <= ceiling);
    fs::read(path).unwrap()
}
fn audit(root: &Path, receiver: &ViewReceiver, revision: u64) {
    let source =
        CaptureArtifact::read(root.join(format!("source-vector/whole-{revision:04}.json")))
            .unwrap();
    source.verify_vector_audit(receiver).unwrap();
    assert_eq!(
        source.manifest()["gtk_version"],
        serde_json::json!([4, 22, 4])
    );
    for record in receiver.state().unwrap().records.values() {
        if let Record::Text(text) = record {
            assert!(!text
                .content
                .as_ref()
                .is_some_and(|text| text.contains(PREEDIT)));
        }
    }
}
#[derive(Default)]
struct Replay {
    packets: usize,
    results: Vec<packet::CommandResult>,
    commands: Vec<core::TextComposition>,
    revisions: BTreeMap<Key, Vec<u64>>,
    observer_packets: usize,
    grants: Vec<packet::ControlGranted>,
    revocations: Vec<packet::ControlRevoked>,
    installed_surface_tokens: BTreeMap<(Key, u64), Vec<Vec<u8>>>,
}
fn replay(root: &Path, phase: &str, evidence: &Value) -> Replay {
    let trace = read(&root.join(format!("{phase}.trace.jsonl")), 16 << 20);
    let mut rows = std::str::from_utf8(&trace)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap());
    let mut replay = Replay::default();
    let mut segment = 0;
    while let Some(scope) = rows.next() {
        segment += 1;
        assert_eq!(scope["record"], "scope");
        assert_eq!(scope["segment"], segment);
        let context = ResourceContext {
            scope: ResourceScope {
                session_id: hex(&scope["session_id"]),
                epoch: hex(&scope["epoch"]),
                principal_id: hex(&scope["principal_id"]),
            },
            view_id: hex(&scope["view_id"]),
        };
        let observer = context.scope.principal_id[..] == Sha256::digest(b"composition-62")[..16];
        if !observer {
            assert_eq!(
                context.scope.principal_id[..],
                Sha256::digest(b"composition-61")[..16]
            );
        }
        let mut receiver = ViewReceiver::new(
            context,
            ReceiverLimits {
                view_memory_bytes: 64 << 20,
                envelope_bytes: 128 << 10,
                chunk_bytes: 32 << 10,
                ..Default::default()
            },
            ReceiverConfig {
                single_glyph_outlines: true,
                ..Default::default()
            },
        )
        .unwrap();
        for sequence in 1u64.. {
            let entry = rows.next().expect("missing segment end");
            assert_eq!(entry["segment"], segment);
            if entry["record"] == "segment-end" {
                assert_eq!(entry["through_sequence"], sequence - 1);
                break;
            }
            assert_eq!(entry["record"], "packet");
            assert_eq!(entry["sequence"], sequence);
            replay_packet(root, &mut replay, &mut receiver, context, observer, &entry);
        }
        receiver.close();
        assert_eq!(receiver.memory_used(), 0);
    }
    assert_eq!(evidence["post_auth_packets"], replay.packets);
    assert_eq!(
        evidence["observer_packets_without_preedit"],
        replay.observer_packets
    );
    assert!(replay.observer_packets > 0);
    replay
}

fn replay_packet(
    root: &Path,
    replay: &mut Replay,
    receiver: &mut ViewReceiver,
    context: ResourceContext,
    observer: bool,
    entry: &Value,
) {
    let key = (context.scope.principal_id, context.view_id);
    let encoded = entry["exact_packet_hex"].as_str().unwrap();
    assert!(encoded.len() <= 2 * (128 << 10));
    let bytes = HEXLOWER.decode(encoded.as_bytes()).unwrap();
    replay.packets += 1;
    assert_eq!(entry["wire_bytes"], bytes.len());
    assert_eq!(entry["sha256"], format!("{:x}", Sha256::digest(&bytes)));
    let message = packet::Packet::decode(bytes.as_slice()).unwrap();
    assert_eq!(message.protocol_version, 1);
    assert_eq!(message.session_id, context.scope.session_id);
    assert_eq!(message.epoch, context.scope.epoch);
    assert_eq!(message.view_id, context.view_id);
    let kind = packet::Kind::try_from(message.kind).unwrap();
    assert!(!matches!(
        kind,
        packet::Kind::ClientHello | packet::Kind::ServerHello
    ));
    assert_eq!(entry["kind"], message.kind);
    if observer {
        replay.observer_packets += 1;
        assert!(!bytes
            .windows(PREEDIT.len())
            .any(|bytes| bytes == PREEDIT.as_bytes()));
        assert_ne!(kind, packet::Kind::TextComposition);
    }
    if entry["direction"] == "client-to-host" {
        if kind == packet::Kind::TextComposition {
            replay
                .commands
                .push(core::TextComposition::decode(message.payload.as_slice()).unwrap());
        }
        return;
    }
    assert_eq!(entry["direction"], "host-to-client");
    let outcome = match kind {
        packet::Kind::SnapshotBegin => {
            let begin = packet::PublicationBegin::decode(message.payload.as_slice()).unwrap();
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
        packet::Kind::PublicationChunk => {
            let chunk = packet::PublicationChunk::decode(message.payload.as_slice()).unwrap();
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
        packet::Kind::PublicationCommit => {
            let commit = packet::PublicationCommit::decode(message.payload.as_slice()).unwrap();
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
        packet::Kind::ResourceOffer => {
            let offer = packet::ResourceOffer::decode(message.payload.as_slice()).unwrap();
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
        packet::Kind::ResourceChunk => {
            let chunk = packet::ResourceChunk::decode(message.payload.as_slice()).unwrap();
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
        packet::Kind::ResourceComplete => {
            let complete = packet::ResourceComplete::decode(message.payload.as_slice()).unwrap();
            receiver
                .complete_resource(context, complete.digest.try_into().unwrap())
                .unwrap()
        }
        packet::Kind::CommandResult => {
            replay
                .results
                .push(packet::CommandResult::decode(message.payload.as_slice()).unwrap());
            None
        }
        packet::Kind::ControlGranted => {
            replay
                .grants
                .push(packet::ControlGranted::decode(message.payload.as_slice()).unwrap());
            None
        }
        packet::Kind::ControlRevoked => {
            replay
                .revocations
                .push(packet::ControlRevoked::decode(message.payload.as_slice()).unwrap());
            None
        }
        packet::Kind::ViewOpened => {
            let opened = packet::ViewOpened::decode(message.payload.as_slice()).unwrap();
            assert_eq!(
                opened
                    .granted_input_modes
                    .iter()
                    .any(|mode| mode == "text.client.v1"),
                !observer
            );
            assert!(opened.input_devices.is_empty());
            None
        }
        _ => None,
    };
    if let Some(ReceiveOutcome::Applied { revision }) = outcome {
        audit(root, receiver, revision);
        replay.revisions.entry(key).or_default().push(revision);
        let state = receiver.state().unwrap();
        let session = state
            .records
            .values()
            .find_map(|record| match record {
                Record::Session(session) => Some(session),
                _ => None,
            })
            .unwrap();
        let Record::Surface(surface) = &state.records[&session.focus_surface.unwrap().id] else {
            panic!("focused surface")
        };
        replay
            .installed_surface_tokens
            .insert((key, revision), surface.source_capture_tokens.clone());
    }
}

#[test]
fn actual_native_transient_stages_commit_retry_and_each_cancellation_replay_under_exact_scopes() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gtk");
    let report: Value =
        serde_json::from_slice(&read(&base.join("composition-interaction.json"), 8 << 20)).unwrap();
    let root = base.join("composition-golden");
    assert!(report["presentation_scope"]
        .as_str()
        .unwrap()
        .contains("simulated"));
    assert_eq!(report["pinned_gtk"], serde_json::json!([4, 22, 4]));
    let expected = [
        "commit",
        "cancel",
        "focus",
        "selection",
        "removal",
        "revocation",
        "disconnect",
        "renewal",
        "expiry",
        "clock-insertion",
        "clock-deletion",
        "clock-noop",
        "clock-empty",
        "clock-prefix",
        "clock-empty-noop",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    assert_eq!(
        report["scenarios"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        expected
    );
    let mut files = BTreeSet::new();
    inventory(&root, &root, &mut files);
    assert_eq!(
        files,
        report["retained_trace_sha256"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    );
    let mut retained = 0usize;
    for (name, digest) in report["retained_trace_sha256"].as_object().unwrap() {
        let bytes = read(&root.join(name), 16 << 20);
        retained += bytes.len();
        assert_eq!(
            format!("{:x}", Sha256::digest(bytes)),
            digest.as_str().unwrap()
        );
        assert!(matches!(
            root.join(name).extension().unwrap().to_str().unwrap(),
            "json" | "jsonl" | "bin" | "rvp"
        ));
    }
    assert_eq!(report["retained_bytes"], retained);
    assert!(retained < 40 << 20);
    for (name, evidence) in report["scenarios"].as_object().unwrap() {
        let directory = root.join(name);
        assert_eq!(evidence["host_memory_after_disposal"], 0);
        assert!(evidence["host_reservation_peak_bytes"].as_u64().unwrap() <= 256 << 20);
        assert_eq!(evidence["transient_public_state_unchanged"], true);
        assert_eq!(evidence["observer_state_unchanged"], true);
        assert_eq!(evidence["private_preedit_diagnostic"]["active"], true);
        assert_eq!(evidence["private_preedit_diagnostic"]["preedit"], PREEDIT);
        assert_eq!(
            evidence["private_preedit_diagnostic"]["scope"],
            "private-controlled-fixture-only"
        );
        assert_eq!(
            evidence["private_preedit_diagnostic"]["source_generation"],
            1
        );
        let before = replay(
            &directory,
            "protocol-before",
            &evidence["post_auth_before_lifecycle"],
        );
        assert_eq!(before.commands.len(), 2);
        assert_eq!(before.grants.len(), 1);
        let lease = &before.grants[0];
        for command in &before.commands {
            let header = command.header.as_ref().unwrap();
            assert_eq!(header.control_lease_id, lease.lease_id);
            assert_eq!(header.control_generation, lease.generation);
            assert_eq!(header.observed_view_revision, 2);
            assert_eq!(command.target_id, before.commands[0].target_id);
            assert_eq!(command.text_session_id, before.commands[0].text_session_id);
            assert_eq!(
                command.focus_generation,
                before.commands[0].focus_generation
            );
            assert_eq!(
                command.base_text_version,
                before.commands[0].base_text_version
            );
        }
        assert!(matches!(
            before.commands[0].operation,
            Some(core::text_composition::Operation::Begin(_))
        ));
        assert!(matches!(
            before.commands[1].operation,
            Some(core::text_composition::Operation::UpdatePreedit(_))
        ));
        assert!(before
            .revisions
            .values()
            .all(|revisions| revisions == &[1, 2]));
        for result in before
            .results
            .iter()
            .filter(|result| result.command_seq == 2 || result.command_seq == 3)
        {
            assert_eq!(result.status, "Dispatched");
            assert!(result.source_commit.is_empty());
            assert!(result.resulting_view_revision.is_none());
        }
        let after = replay(&directory, "protocol-final", &evidence["post_auth_final"]);
        if name == "commit" || name == "renewal" || name.starts_with("clock-") {
            assert_eq!(evidence["exact_commit_retry_equal"], true);
            assert_eq!(evidence["native_commit_once"], true);
            assert_eq!(
                evidence["clock_transition"]["target_surface_causal_witness"],
                true
            );
            let commits = after
                .results
                .iter()
                .filter(|result| result.command_seq == 4)
                .collect::<Vec<_>>();
            assert_eq!(commits.len(), 2);
            assert_eq!(commits[0], commits[1]);
            assert_eq!(commits[0].status, "Applied");
            assert_eq!(commits[0].source_commit, evidence["causal_source_commit"]);
            assert_eq!(commits[0].resulting_view_revision, Some(3));
            assert_eq!(evidence["native_final"]["active"], false);
            assert!(evidence["native_final"]["preedit"].is_null());
            assert_eq!(
                evidence["native_final"]["native_dispatches"]
                    .as_u64()
                    .unwrap(),
                evidence["private_preedit_diagnostic"]["native_dispatches"]
                    .as_u64()
                    .unwrap()
                    + 1
            );
            let commands = after
                .commands
                .iter()
                .filter(|command| command.header.as_ref().unwrap().command_seq == 4)
                .collect::<Vec<_>>();
            assert_eq!(commands.len(), 2);
            assert_eq!(commands[0], commands[1]);
            assert!(matches!(
                commands[0].operation,
                Some(core::text_composition::Operation::CommitText(_))
            ));
            assert!(after
                .revisions
                .values()
                .all(|revisions| revisions == &[1, 2, 3]));
            assert_eq!(after.revisions.len(), 2);
            for key in after.revisions.keys() {
                assert!(after.installed_surface_tokens[&(*key, 3)]
                    .iter()
                    .any(|token| token == commits[0].source_commit.as_bytes()));
            }
            let source =
                CaptureArtifact::read(directory.join("source-vector/whole-0003.json")).unwrap();
            let modal = source.manifest()["surfaces"]
                .as_array()
                .unwrap()
                .iter()
                .find(|surface| surface["surface_key"] == "modal")
                .unwrap();
            assert_eq!(modal["capture_token"], commits[0].source_commit);
            let prior =
                CaptureArtifact::read(directory.join("source-vector/whole-0002.json")).unwrap();
            let text = |artifact: &CaptureArtifact| {
                artifact.manifest()["semantics"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|node| node["identity"] == "modal-entry")
                    .unwrap()["text"]
                    .clone()
            };
            let before_text = text(&prior);
            let after_text = text(&source);
            let Some(core::text_composition::Operation::Begin(range)) =
                before.commands[0].operation.as_ref()
            else {
                unreachable!()
            };
            let Some(core::text_composition::Operation::CommitText(commit)) =
                commands[0].operation.as_ref()
            else {
                unreachable!()
            };
            let expected_text = before_text["content"]
                .as_str()
                .unwrap()
                .chars()
                .take(range.start as usize)
                .collect::<String>()
                + &commit.text
                + &before_text["content"]
                    .as_str()
                    .unwrap()
                    .chars()
                    .skip(range.end as usize)
                    .collect::<String>();
            assert_eq!(after_text["content"], expected_text);
            let caret = after_text["selection"][1].as_u64().unwrap();
            assert_eq!(caret, range.start + commit.text.chars().count() as u64);
            assert_eq!(after_text["selection"][0], caret);
            let content_delta = u64::from(!before_text["content"].as_str().unwrap().is_empty())
                + u64::from(!after_text["content"].as_str().unwrap().is_empty());
            assert_eq!(
                after_text["text_version"].as_u64().unwrap()
                    - before_text["text_version"].as_u64().unwrap(),
                content_delta
            );
            assert_eq!(
                after_text["version"].as_u64().unwrap() - before_text["version"].as_u64().unwrap(),
                content_delta + u64::from(caret > 0)
            );
            if name == "renewal" {
                assert_eq!(
                    evidence["held_preedit_across_original_expiry_and_exact_old_renew"],
                    true
                );
                assert_eq!(after.grants.len(), 4);
                assert_eq!(after.grants[1], after.grants[3]);
                assert!(after.grants[2].expires_at_ms > after.grants[1].expires_at_ms);
                assert!(after.grants[1].expires_at_ms > after.grants[0].expires_at_ms);
                assert!(after
                    .grants
                    .iter()
                    .all(|grant| grant.lease_id == lease.lease_id
                        && grant.generation == lease.generation));
            }
        } else {
            assert_eq!(evidence["cancellation"]["active"], false);
            assert!(evidence["cancellation"]["preedit"].is_null());
            assert_eq!(evidence["late_commit_denied_without_native_dispatch"], true);
            let cause = match name.as_str() {
                "cancel" => "explicit-cancel",
                "focus" => "stale-native-composition-focus",
                "selection" => "stale-native-composition-text-record",
                "removal" => "retired-source-object",
                "disconnect" | "revocation" | "expiry" => "retired-native-lifecycle-fence",
                _ => unreachable!(),
            };
            assert_eq!(evidence["cancellation"]["last_cancellation"], cause);
            assert_eq!(evidence["cancellation"]["cancelled"], 1);
            assert_eq!(
                evidence["cancellation"]["native_dispatches"]
                    .as_u64()
                    .unwrap(),
                evidence["private_preedit_diagnostic"]["native_dispatches"]
                    .as_u64()
                    .unwrap()
                    + u64::from(name == "cancel")
            );
            let sequence = if name == "cancel" { 5 } else { 4 };
            assert!(after
                .results
                .iter()
                .any(|result| result.command_seq == sequence
                    && !matches!(result.status.as_str(), "Applied" | "Dispatched")));
            assert!(after
                .revisions
                .values()
                .all(|revisions| revisions.iter().all(|revision| *revision <= 2)));
            if name == "cancel" {
                let cancelled = after
                    .results
                    .iter()
                    .filter(|result| result.command_seq == 4)
                    .collect::<Vec<_>>();
                assert_eq!(cancelled.len(), 2);
                assert_eq!(cancelled[0], cancelled[1]);
                assert_eq!(cancelled[0].status, "Dispatched");
            }
            if matches!(name.as_str(), "disconnect" | "revocation" | "expiry") {
                assert_eq!(evidence["cancellation"]["fence_generation"], 0);
            }
            if name == "expiry" {
                assert_eq!(after.revocations.len(), 1);
                assert_eq!(after.revocations[0].lease_id, lease.lease_id);
                assert_eq!(after.revocations[0].generation, lease.generation);
                assert_eq!(after.revocations[0].reason, "NativeLifecycle");
            }
        }
    }
}
