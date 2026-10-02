//! Fixture-only JSONL harness: payloads are exact Protobuf bytes encoded as hex.
//! The JSON wrapper is a local conformance interface, not a RUI transport.
use rui_protocol_core::model::{Limits, Replica};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::{self, BufRead};

fn unhex(text: &str) -> Result<Vec<u8>, &'static str> {
    if !text.len().is_multiple_of(2) || text.len() > 32 * 1024 * 1024 {
        return Err("invalid hex length");
    }
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16).ok_or("invalid hex")?;
            let low = (pair[1] as char).to_digit(16).ok_or("invalid hex")?;
            Ok(((high << 4) | low) as u8)
        })
        .collect()
}

fn normalized(replica: &Replica) -> Value {
    let state = serde_json::to_value(replica.state()).expect("fixture state serializes");
    let mut records = Vec::new();
    for record in state["records"].as_object().expect("record map").values() {
        let node = record.get("Semantic").expect("semantic-only fixture");
        let children: Vec<_> = node["children"]
            .as_array()
            .expect("child list")
            .iter()
            .map(|child| child["id"].clone())
            .collect();
        records.push(json!({
            "id": node["id"], "version": node["version"],
            "role": node["role"], "sensitivity": node["sensitivity"],
            "completeness": node["completeness"],
            "label": node["label"], "description": node["description"],
            "children": children,
        }));
    }
    records.sort_by_key(|record| record["id"].as_u64().expect("numeric id"));
    json!({"revision":state["revision"], "retired":state["retired"], "records":records})
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut replica = Replica::new(Limits::default());
    let available = BTreeSet::new();
    for line in io::stdin().lock().lines() {
        let request: Value = serde_json::from_str(&line?)?;
        let bytes = unhex(request["hex"].as_str().ok_or("missing payload")?)?;
        let outcome = match request["kind"].as_str() {
            Some("snapshot") => replica.apply_snapshot(&bytes, &available),
            Some("transaction") => replica.apply_transaction(&bytes, &available),
            _ => return Err("unknown fixture operation".into()),
        };
        println!(
            "{}",
            json!({"accepted":outcome.is_ok(), "state":normalized(&replica)})
        );
    }
    Ok(())
}
