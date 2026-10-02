//! Deterministic mutation fuzzing and replay properties. Seeds and failing
//! operation indexes make every counterexample reproducible; the first failing
//! prefix is already a reduced transcript for diagnosis.
use prost::Message;
use rui_protocol_core::model::{ApplyOutcome, Limits, Replica};
use rui_protocol_core::resources::{validate_vector_bytes, MemoryBudget, ResourceLimits};
use rui_protocol_core::{
    commands,
    wire::{self, proto as p},
};
use std::collections::BTreeSet;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}
fn semantic(id: u64) -> p::Record {
    p::Record {
        record: Some(p::record::Record::Semantic(p::SemanticNode {
            id,
            version: 1,
            role: if id == 1 {
                p::SemanticRole::Root
            } else {
                p::SemanticRole::Label
            } as i32,
            label: Some(format!("node-{id}")),
            sensitivity: p::Sensitivity::Public as i32,
            completeness: p::Completeness::Full as i32,
            ordered_children: if id == 1 {
                (2..=21)
                    .map(|id| p::ObjectRef {
                        kind: p::ObjectKind::Semantic as i32,
                        id,
                    })
                    .collect()
            } else {
                vec![]
            },
            ..Default::default()
        })),
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

#[test]
fn pro_001_generated_replay_duplicates_rejections_and_fresh_snapshots_agree() {
    for seed in 1..=32 {
        let mut rng = Rng(seed);
        let mut records: Vec<p::Record> = (1..=21).map(semantic).collect();
        let mut incremental = Replica::new(Limits::default());
        incremental
            .apply_snapshot(&snapshot(1, records.clone()), &BTreeSet::new())
            .unwrap();
        for base in 1..=64 {
            let target = (rng.next() % 20 + 1) as usize;
            if let Some(p::record::Record::Semantic(node)) = &mut records[target].record {
                node.version += 1;
                node.label = Some(format!("seed-{seed}-edit-{base}-😀"));
            }
            let mut changed = vec![records[target].clone()];
            if rng.next().is_multiple_of(3) {
                if let Some(p::record::Record::Semantic(root)) = &mut records[0].record {
                    root.version += 1;
                    root.ordered_children.reverse();
                }
                changed.push(records[0].clone());
            }
            let tx = p::Transaction {
                base_revision: base,
                new_revision: base + 1,
                mutations: changed
                    .into_iter()
                    .map(|v| p::Mutation {
                        mutation: Some(p::mutation::Mutation::Put(v)),
                    })
                    .collect(),
                ..Default::default()
            };
            // Invalid version precedes a valid retry, preserving the entire old graph.
            let mut malformed = tx.clone();
            if let Some(p::mutation::Mutation::Put(p::Record {
                record: Some(p::record::Record::Semantic(node)),
            })) = &mut malformed.mutations[0].mutation
            {
                node.version += 1;
            }
            let before = incremental.state().clone();
            assert!(incremental
                .apply_transaction(&malformed.encode_to_vec(), &BTreeSet::new())
                .is_err());
            assert_eq!(incremental.state(), &before, "seed {seed}, prefix {base}");
            let bytes = tx.encode_to_vec();
            incremental
                .apply_transaction(&bytes, &BTreeSet::new())
                .unwrap();
            assert_eq!(
                incremental
                    .apply_transaction(&bytes, &BTreeSet::new())
                    .unwrap(),
                ApplyOutcome::Duplicate { revision: base + 1 }
            );
            let mut fresh = Replica::new(Limits::default());
            fresh
                .apply_snapshot(&snapshot(base + 1, records.clone()), &BTreeSet::new())
                .unwrap();
            assert_eq!(
                incremental.state(),
                fresh.state(),
                "seed {seed}, prefix {base}"
            );
        }
    }
}

#[test]
fn sec_004_mutation_fuzzing_preserves_atomicity_and_allocation_bounds() {
    let limits = Limits {
        max_envelope_bytes: 4096,
        max_snapshot_bytes: 4096,
        max_transaction_bytes: 4096,
        max_memory_bytes: 256 * 1024,
        max_records: 32,
        max_depth: 8,
        max_gradient_stops: 32,
        max_dash_segments: 32,
        ..Default::default()
    };
    let vector_limits = ResourceLimits {
        max_resource_bytes: 4096,
        max_vector_commands: 128,
        ..Default::default()
    };
    let seed_snapshot = snapshot(1, vec![semantic(2)]);
    let mut rng = Rng(0x52554920261002);
    for iteration in 0..10_000 {
        let mut bytes = if iteration % 2 == 0 {
            seed_snapshot.clone()
        } else {
            vec![0; (rng.next() % 512) as usize]
        };
        for _ in 0..1 + rng.next() % 8 {
            if bytes.is_empty() {
                bytes.push(rng.next() as u8);
            }
            let at = rng.next() as usize % bytes.len();
            bytes[at] = rng.next() as u8;
        }
        if iteration % 7 == 0 {
            bytes.truncate(rng.next() as usize % (bytes.len() + 1));
        }
        let _ = wire::decode_envelope(&bytes, &limits);
        let _ = wire::decode_transaction(&bytes, &limits);
        let _ = commands::decode_command(&bytes);
        let _ = validate_vector_bytes(&bytes, &vector_limits);
        let budget = MemoryBudget::new(limits.max_memory_bytes);
        let mut replica = Replica::with_memory_budget(limits.clone(), budget.clone());
        let old = replica.state().clone();
        if replica.apply_snapshot(&bytes, &BTreeSet::new()).is_err() {
            assert_eq!(replica.state(), &old, "mutation {iteration}");
        }
        assert!(budget.used() <= budget.limit());
        drop(replica);
        assert_eq!(budget.used(), 0, "mutation {iteration}");
    }
}
