use prost::Message;
use rui_producer_sdk::publication_builder::{build, BuildError};
use rui_protocol_core::{
    model::{ApplyOutcome, Limits, Replica},
    wire::proto as p,
};
use std::{collections::BTreeSet, path::PathBuf};

fn fixture(name: &str) -> p::Snapshot {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gtk");
    let bytes = std::fs::read(root.join(name)).unwrap();
    let envelope = p::Envelope::decode(bytes.as_slice()).unwrap();
    p::Snapshot::decode(envelope.payload.as_slice()).unwrap()
}
fn available(s: &p::Snapshot) -> BTreeSet<[u8; 32]> {
    s.required_resources
        .iter()
        .map(|d| d.as_slice().try_into().unwrap())
        .collect()
}

#[test]
fn coalesced_native_sequence_replays_all_records_and_preserves_retirements() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/gtk/whole-native-golden/whole-native");
    let mut paths: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("snapshot-")
                && p.extension().unwrap() == "bin"
        })
        .collect();
    paths.sort();
    let mut previous = fixture(&format!(
        "whole-native-golden/whole-native/{}",
        paths[0].file_name().unwrap().to_str().unwrap()
    ));
    let mut reducer = Replica::new(Limits::default());
    reducer
        .apply_snapshot(&previous.encode_to_vec(), &available(&previous))
        .unwrap();
    // Skip captures deliberately, then keep each exact public step unchanged.
    for path in paths.into_iter().skip(1).step_by(2) {
        let e = p::Envelope::decode(std::fs::read(path).unwrap().as_slice()).unwrap();
        let source = p::Snapshot::decode(e.payload.as_slice()).unwrap();
        let candidate = build(&previous, source, &reducer.state().retired).unwrap();
        candidate
            .validate(&reducer, &available(&candidate.snapshot), reducer.limits())
            .unwrap();
        let retired = reducer.state().retired.clone();
        assert!(matches!(
            reducer
                .apply_transaction(
                    &candidate.transaction.encode_to_vec(),
                    &available(&candidate.snapshot)
                )
                .unwrap(),
            ApplyOutcome::Applied { .. }
        ));
        assert!(retired.is_subset(&reducer.state().retired));
        let mut fresh = Replica::new(Limits::default());
        fresh
            .apply_snapshot(
                &candidate.snapshot.encode_to_vec(),
                &available(&candidate.snapshot),
            )
            .unwrap();
        assert_eq!(reducer.state().records, fresh.state().records);
        assert_eq!(reducer.resource_refs(), fresh.resource_refs());
        previous = candidate.snapshot;
    }
    assert!(!reducer.state().retired.is_empty());
}

#[test]
fn selection_content_and_source_clock_leaps_are_normalized_independently() {
    let previous = fixture("unicode-golden/unicode/snapshot-000001.bin");
    let mut source = previous.clone();
    source.revision += 300;
    let target = source
        .records
        .iter_mut()
        .find_map(|r| {
            if let Some(p::record::Record::Text(t)) = &mut r.record {
                Some(t)
            } else {
                None
            }
        })
        .unwrap();
    let id = target.id;
    let old_record = target.version;
    let old_text = target.text_version;
    target.version += 700;
    target.text_version += 500;
    target.selection = Some(p::TextRange {
        anchor: 0,
        focus: 0,
    });
    let candidate = build(&previous, source, &BTreeSet::new()).unwrap();
    let text = candidate
        .snapshot
        .records
        .iter()
        .find_map(|r| {
            if let Some(p::record::Record::Text(t)) = &r.record {
                (t.id == id).then_some(t)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(text.text_version, old_text);
    assert!(text.version == old_record || text.version == old_record + 1);
    assert_eq!(candidate.source_clocks[&id].text, Some(old_text + 500));
    let mut source = candidate.snapshot.clone();
    let text = source
        .records
        .iter_mut()
        .find_map(|r| {
            if let Some(p::record::Record::Text(t)) = &mut r.record {
                (t.id == id).then_some(t)
            } else {
                None
            }
        })
        .unwrap();
    text.content = Some("Unicode é😀e\u{301}".into());
    text.version += 100;
    text.text_version += 100;
    let next = build(&candidate.snapshot, source, &BTreeSet::new()).unwrap();
    let text = next
        .snapshot
        .records
        .iter()
        .find_map(|r| {
            if let Some(p::record::Record::Text(t)) = &r.record {
                (t.id == id).then_some(t)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(text.text_version, old_text + 1);
}

#[test]
fn missing_resources_cannot_make_two_uncommitted_replays_look_equal() {
    let previous = fixture("whole-native-golden/whole-native/snapshot-01-000001.bin");
    let mut reducer = Replica::new(Limits::default());
    reducer
        .apply_snapshot(&previous.encode_to_vec(), &available(&previous))
        .unwrap();
    let candidate = build(&previous, previous.clone(), &BTreeSet::new()).unwrap();
    assert!(matches!(
        candidate.validate(&reducer, &BTreeSet::new(), reducer.limits()),
        Err(BuildError::Divergence)
    ));
    let id = previous.records[0].record.as_ref().unwrap();
    let id = match id {
        p::record::Record::Surface(r) => r.id,
        p::record::Record::Scene(r) => r.id,
        p::record::Record::Semantic(r) => r.id,
        p::record::Record::Text(r) => r.id,
        p::record::Record::Binding(r) => r.id,
        p::record::Record::Session(r) => r.id,
    };
    assert!(build(&previous, previous.clone(), &[id].into_iter().collect()).is_err());
    let mut exhausted = previous.clone();
    exhausted.revision = u64::MAX;
    assert!(matches!(
        build(&exhausted, previous, &BTreeSet::new()),
        Err(BuildError::Clock)
    ));
}

#[test]
fn every_transaction_chunk_cut_preserves_predecessor_and_exact_retry() {
    use rui_protocol_core::{
        publication::{publication_digest, PublicationBegin, PublicationCommit, PublicationKind},
        receiver::{ReceiverConfig, ViewReceiver},
        resources::{ResourceContext, ResourceScope},
        session::ReceiverLimits,
    };
    let context = ResourceContext {
        scope: ResourceScope {
            session_id: [1; 16],
            epoch: [2; 16],
            principal_id: [4; 16],
        },
        view_id: [3; 16],
    };
    let snapshot = p::Snapshot {
        revision: 1,
        records: vec![p::Record {
            record: Some(p::record::Record::Semantic(p::SemanticNode {
                browser_input_type: None,
                id: 1,
                version: 1,
                role: p::SemanticRole::Root as i32,
                sensitivity: p::Sensitivity::Public as i32,
                completeness: p::Completeness::Full as i32,
                ..Default::default()
            })),
        }],
        ..Default::default()
    };
    let mut target = snapshot.clone();
    if let Some(p::record::Record::Semantic(node)) = &mut target.records[0].record {
        node.description = Some("x".repeat(80_000));
    }
    let candidate = build(&snapshot, target, &BTreeSet::new()).unwrap();
    let bytes = candidate.transaction.encode_to_vec();
    let chunks: Vec<_> = bytes.chunks(4096).collect();
    let begin = PublicationBegin {
        publication_id: [7; 16],
        kind: PublicationKind::Transaction {
            base_revision: 1,
            new_revision: 2,
        },
        total_bytes: bytes.len() as u64,
        chunk_count: chunks.len() as u32,
        item_count: 1,
        digest: publication_digest(&bytes),
    };
    for cut in 0..=chunks.len() {
        let mut peer = ViewReceiver::new(
            context,
            ReceiverLimits {
                envelope_bytes: 4096,
                chunk_bytes: 4096,
                view_memory_bytes: 8 << 20,
                ..Default::default()
            },
            ReceiverConfig::default(),
        )
        .unwrap();
        let initial = p::Envelope {
            protocol_version: 1,
            session_id: [1; 16].to_vec(),
            epoch: [2; 16].to_vec(),
            view_id: [3; 16].to_vec(),
            kind: p::MessageKind::Snapshot as i32,
            payload: snapshot.encode_to_vec(),
            required_features: vec![],
        }
        .encode_to_vec();
        peer.receive_envelope([6; 16], &initial).unwrap();
        peer.begin_publication(context, begin.clone()).unwrap();
        for (index, chunk) in chunks.iter().take(cut).enumerate() {
            peer.publication_chunk(context, [7; 16], index as u32, chunk)
                .unwrap();
        }
        assert_eq!(peer.state().unwrap().revision, 1);
        peer.abort_publication(context, [7; 16]).unwrap();
        peer.begin_publication(context, begin.clone()).unwrap();
        for (index, chunk) in chunks.iter().enumerate() {
            peer.publication_chunk(context, [7; 16], index as u32, chunk)
                .unwrap();
        }
        peer.commit_publication(context, [7; 16], PublicationCommit::from(&begin))
            .unwrap();
        assert_eq!(peer.state().unwrap().revision, 2);
        assert!(peer.memory_used() <= peer.memory_limit());
    }
}

#[test]
fn clock_only_source_changes_preserve_records_even_at_clock_limits() {
    let previous = p::Snapshot {
        revision: 1,
        records: vec![
            p::Record {
                record: Some(p::record::Record::Semantic(p::SemanticNode {
                    id: 1,
                    version: u64::MAX,
                    collection_model: Some(p::CollectionModel {
                        version: u64::MAX,
                        total_count: Some(0),
                        ..Default::default()
                    }),
                    ..Default::default()
                })),
            },
            p::Record {
                record: Some(p::record::Record::Session(p::SessionState {
                    id: 2,
                    version: u64::MAX,
                    focus_generation: u64::MAX,
                    ..Default::default()
                })),
            },
        ],
        ..Default::default()
    };
    let mut source = previous.clone();
    for record in &mut source.records {
        match record.record.as_mut().unwrap() {
            p::record::Record::Semantic(r) => {
                r.version = 1;
                r.collection_model.as_mut().unwrap().version = 1;
            }
            p::record::Record::Session(r) => {
                r.version = 1;
                r.focus_generation = 1;
            }
            _ => unreachable!(),
        }
    }
    let candidate = build(&previous, source.clone(), &BTreeSet::new()).unwrap();
    assert_eq!(candidate.snapshot.records, previous.records);
    assert!(candidate.transaction.mutations.is_empty());
    let Some(p::record::Record::Semantic(r)) = &mut source.records[0].record else {
        unreachable!()
    };
    r.collection_model.as_mut().unwrap().total_count = Some(1);
    assert!(matches!(
        build(&previous, source, &BTreeSet::new()),
        Err(BuildError::Clock)
    ));
}
