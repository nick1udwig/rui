//! Synthetic scene conformance interface, not an authenticated network endpoint.
use rui_protocol_core::model::{ApplyOutcome, Limits, Replica};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::{self, BufRead};

fn decode_hex(text: &str) -> Result<Vec<u8>, &'static str> {
    if !text.len().is_multiple_of(2) || text.len() > 2 * 1024 * 1024 {
        return Err("invalid fixture length");
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Synthetic resource availability only. This fixture does not load or render fonts.
    let available = BTreeSet::from([[1; 32], [2; 32], [3; 32]]);
    for line in io::stdin().lock().lines() {
        let request: Value = serde_json::from_str(&line?)?;
        let bytes = decode_hex(request["hex"].as_str().ok_or("missing payload")?)?;
        let mut replica = Replica::new(Limits::default());
        let outcome = replica.apply_snapshot(&bytes, &available);
        println!(
            "{}",
            json!({
                "accepted":matches!(outcome, Ok(ApplyOutcome::Applied { .. })),
                "state":replica.state(),
            })
        );
    }
    Ok(())
}
