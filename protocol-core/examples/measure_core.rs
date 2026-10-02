//! Reproducible CPU-only reference workload. Does not claim network/render latency.
use prost::Message;
use rui_protocol_core::model::{Limits, Replica};
use rui_protocol_core::resources::MemoryBudget;
use rui_protocol_core::wire::proto as p;
use serde_json::json;
use std::collections::BTreeSet;
use std::time::Instant;

fn record(id: u64, version: u64) -> p::Record {
    p::Record {
        record: Some(p::record::Record::Semantic(p::SemanticNode {
            id,
            version,
            role: p::SemanticRole::Label as i32,
            label: Some(format!("record-{id}-version-{version}")),
            sensitivity: p::Sensitivity::Public as i32,
            completeness: p::Completeness::Full as i32,
            ..Default::default()
        })),
    }
}
fn summary(mut samples: Vec<u128>) -> serde_json::Value {
    samples.sort_unstable();
    let percentile = |p: usize| samples[(samples.len() * p).div_ceil(100).saturating_sub(1)];
    json!({"unit":"microseconds","samples":samples.len(),"p50":percentile(50),"p95":percentile(95),"p99":percentile(99)})
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let count = 10_000u64;
    let mut versions = vec![1u64; count as usize];
    let budget = MemoryBudget::new(128 << 20);
    let mut replica = Replica::with_memory_budget(Limits::default(), budget.clone());
    let start = Instant::now();
    let snapshot = p::Snapshot {
        revision: 1,
        records: (1..=count).map(|id| record(id, 1)).collect(),
        ..Default::default()
    }
    .encode_to_vec();
    let snapshot_encode = start.elapsed().as_micros();
    let start = Instant::now();
    replica.apply_snapshot(&snapshot, &BTreeSet::new())?;
    let snapshot_apply = start.elapsed().as_micros();
    let mut rows = Vec::new();
    let mut revision = 1;
    for changed in [1u64, 20] {
        let mut encode = Vec::new();
        let mut apply = Vec::new();
        let mut wire_bytes = Vec::new();
        for iteration in 0..110u64 {
            let start = Instant::now();
            let mutations = (0..changed)
                .map(|offset| {
                    let id = (iteration * 23 + offset) % count + 1;
                    versions[id as usize - 1] += 1;
                    p::Mutation {
                        mutation: Some(p::mutation::Mutation::Put(record(
                            id,
                            versions[id as usize - 1],
                        ))),
                    }
                })
                .collect();
            let transaction = p::Transaction {
                base_revision: revision,
                new_revision: revision + 1,
                mutations,
                ..Default::default()
            }
            .encode_to_vec();
            let encode_us = start.elapsed().as_micros();
            let start = Instant::now();
            replica.apply_transaction(&transaction, &BTreeSet::new())?;
            let apply_us = start.elapsed().as_micros();
            revision += 1;
            if iteration >= 10 {
                encode.push(encode_us);
                apply.push(apply_us);
                wire_bytes.push(transaction.len());
            }
        }
        rows.push(json!({"changed_records":changed,"encode":summary(encode),"apply":summary(apply),"wire_bytes_min":wire_bytes.iter().min(),"wire_bytes_max":wire_bytes.iter().max()}));
    }
    let retained = budget.used();
    drop(replica);
    assert_eq!(budget.used(), 0);
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"records":count,"profile":"core-cpu-only","snapshot_bytes":snapshot.len(),
        "snapshot_encode_us":snapshot_encode,"snapshot_apply_us":snapshot_apply,"updates":rows,"retained_bytes":retained,"released_bytes_after_drop":budget.used(),
        "resource_bodies_sent":0,"note":"Semantic records have no resource closure; cache hit rates and rendering/network latency require the integrated benchmark."})
        )?
    );
    Ok(())
}
