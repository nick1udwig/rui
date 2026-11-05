//! Export the game's actual projection and incremental builder for offline comparison.
#[path = "../projection.rs"]
mod projection;
use prost::Message;
use rui_producer_sdk::publication_builder;
use rui_protocol_core::{
    model::{Limits, Replica},
    wire::proto as p,
};
use serde_json::json;
use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufRead, BufReader, Write},
    time::Instant,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err("usage: game-trace FRAMES_JSONL OUTPUT_BINARY OUTPUT_JSON".into());
    }
    let mut output = File::create(&args[2])?;
    let mut previous = p::Snapshot::default();
    let mut replica = Replica::new(Limits::default());
    let mut slots = 0;
    let mut samples = Vec::new();
    for (index, line) in BufReader::new(File::open(&args[1])?).lines().enumerate() {
        let frame: serde_json::Value = serde_json::from_str(&line?)?;
        let lines: Vec<[f64; 7]> = serde_json::from_value(frame["lines"].clone())?;
        slots = slots.max(lines.len());
        let capture: projection::Capture = serde_json::from_value(json!({
            "nodes": [{"id":3,"version":1,"role":"root","label":"Offline game workload"}],
            "status":"MAME beam comparison",
            "vectors":{"sequence":index+1,"slots":slots,"lines":lines}
        }))?;
        let started = Instant::now();
        let target = projection::snapshot(&capture)?;
        let candidate = publication_builder::build(&previous, target, &BTreeSet::new())?;
        let (kind, payload) = if index == 0 {
            (p::MessageKind::Snapshot, candidate.snapshot.encode_to_vec())
        } else {
            (
                p::MessageKind::Transaction,
                candidate.transaction.encode_to_vec(),
            )
        };
        let envelope = p::Envelope {
            protocol_version: 1,
            session_id: vec![1; 16],
            epoch: vec![2; 16],
            view_id: vec![4; 16],
            kind: kind as i32,
            payload,
            required_features: vec![],
        }
        .encode_to_vec();
        let milliseconds = started.elapsed().as_secs_f64() * 1000.;
        if index == 0 {
            replica.apply_snapshot(&candidate.snapshot.encode_to_vec(), &BTreeSet::new())?;
        } else {
            candidate.validate(&replica, &BTreeSet::new(), &Limits::default())?;
            replica.apply_transaction(&candidate.transaction.encode_to_vec(), &BTreeSet::new())?;
        }
        let scene_bytes: usize = candidate
            .transaction
            .mutations
            .iter()
            .filter(|m| {
                matches!(
                    &m.mutation,
                    Some(p::mutation::Mutation::Put(p::Record {
                        record: Some(p::record::Record::Scene(_))
                    }))
                )
            })
            .map(Message::encoded_len)
            .sum();
        output.write_all(&(envelope.len() as u32).to_be_bytes())?;
        output.write_all(&envelope)?;
        samples.push(json!({"frame":index,"bytes":envelope.len(),"scene_mutation_bytes":scene_bytes,"project_build_encode_ms":milliseconds,"mutations":candidate.transaction.mutations.len(),"lines":lines.len(),"slots":slots}));
        previous = candidate.snapshot;
    }
    if samples.is_empty() {
        return Err("empty capture".into());
    }
    std::fs::write(
        &args[3],
        serde_json::to_vec_pretty(
            &json!({"samples":samples,"controls_included":false,"transport_framing_included":false,"reducer_equivalence_verified":true}),
        )?,
    )?;
    Ok(())
}
