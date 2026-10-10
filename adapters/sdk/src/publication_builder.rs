//! Canonical authorized projections and clock-correct incremental publications.
//! Native clocks are retained separately; public transaction clocks never copy
//! reset-oriented or coalesced source clocks blindly.
use prost::Message;
use rui_protocol_core::{
    model::{ApplyOutcome, Limits, ModelError, Replica},
    wire::proto as p,
};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const MAX_RECORDS: usize = 4096;
const MAX_BYTES: usize = 1 << 20;
#[derive(Debug, Error)]
pub enum BuildError {
    #[error("publication projection exceeds its limit or has invalid identity")]
    Invalid,
    #[error("publication clock exhausted")]
    Clock,
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error("transaction and snapshot replay disagree")]
    Divergence,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceClock {
    pub kind: i32,
    pub version: u64,
    pub text: Option<u64>,
    pub collection: Option<u64>,
    pub focus: Option<u64>,
}
pub struct Candidate {
    pub snapshot: p::Snapshot,
    pub transaction: p::Transaction,
    pub source_clocks: BTreeMap<u64, SourceClock>,
}
fn next(version: u64) -> Result<u64, BuildError> {
    version.checked_add(1).ok_or(BuildError::Clock)
}
fn reference(record: &p::Record) -> Result<p::ObjectRef, BuildError> {
    use p::record::Record::*;
    let (kind, id) = match record.record.as_ref().ok_or(BuildError::Invalid)? {
        Surface(r) => (p::ObjectKind::Surface, r.id),
        Scene(r) => (p::ObjectKind::Scene, r.id),
        Semantic(r) => (p::ObjectKind::Semantic, r.id),
        Text(r) => (p::ObjectKind::Text, r.id),
        Binding(r) => (p::ObjectKind::Binding, r.id),
        Session(r) => (p::ObjectKind::Session, r.id),
    };
    if id == 0 {
        return Err(BuildError::Invalid);
    }
    Ok(p::ObjectRef {
        kind: kind as i32,
        id,
    })
}
pub fn clock(record: &p::Record) -> Result<SourceClock, BuildError> {
    use p::record::Record::*;
    let mut clock = SourceClock {
        kind: reference(record)?.kind,
        version: 0,
        text: None,
        collection: None,
        focus: None,
    };
    clock.version = match record.record.as_ref().unwrap() {
        Surface(r) => r.version,
        Scene(r) => r.version,
        Binding(r) => r.version,
        Text(r) => {
            clock.text = Some(r.text_version);
            r.version
        }
        Semantic(r) => {
            clock.collection = r.collection_model.as_ref().map(|c| c.version);
            r.version
        }
        Session(r) => {
            clock.focus = Some(r.focus_generation);
            r.version
        }
    };
    if clock.version == 0 {
        return Err(BuildError::Invalid);
    }
    Ok(clock)
}
fn version(record: &mut p::Record, version: u64) {
    use p::record::Record::*;
    match record.record.as_mut().unwrap() {
        Surface(r) => r.version = version,
        Scene(r) => r.version = version,
        Semantic(r) => r.version = version,
        Text(r) => r.version = version,
        Binding(r) => r.version = version,
        Session(r) => r.version = version,
    }
}
fn content(record: &p::Record) -> p::Record {
    let mut copy = record.clone();
    version(&mut copy, 0);
    use p::record::Record::*;
    match copy.record.as_mut().unwrap() {
        Text(r) => r.text_version = 0,
        Semantic(r) => {
            if let Some(c) = &mut r.collection_model {
                c.version = 0;
            }
        }
        Session(r) => r.focus_generation = 0,
        _ => {}
    }
    copy
}
fn normalize(target: &mut p::Record, old: Option<&p::Record>) -> Result<(), BuildError> {
    use p::record::Record::*;
    match (
        target.record.as_mut().unwrap(),
        old.and_then(|r| r.record.as_ref()),
    ) {
        (Text(t), Some(Text(o))) => {
            t.text_version = if t.content != o.content || t.status != o.status {
                next(o.text_version)?
            } else {
                o.text_version
            }
        }
        (Text(t), None) => t.text_version = 1,
        (Semantic(t), Some(Semantic(o))) => {
            if let Some(c) = &mut t.collection_model {
                c.version = if let Some(prior) = &o.collection_model {
                    let mut a = c.clone();
                    a.version = 0;
                    let mut b = prior.clone();
                    b.version = 0;
                    if a != b {
                        next(prior.version)?
                    } else {
                        prior.version
                    }
                } else {
                    1
                };
            }
        }
        (Semantic(t), None) => {
            if let Some(c) = &mut t.collection_model {
                c.version = 1;
            }
        }
        (Session(t), Some(Session(o))) => {
            t.focus_generation = if t.focus != o.focus || t.focus_surface != o.focus_surface {
                next(o.focus_generation)?
            } else {
                o.focus_generation
            }
        }
        (Session(t), None) => t.focus_generation = 1,
        _ => {}
    }
    let new = match old {
        Some(old) => {
            let prior = clock(old)?;
            if content(target) == content(old) {
                prior.version
            } else {
                next(prior.version)?
            }
        }
        None => 1,
    };
    version(target, new);
    Ok(())
}
fn inventory(snapshot: &p::Snapshot) -> Result<BTreeMap<u64, &p::Record>, BuildError> {
    if snapshot.records.len() > MAX_RECORDS || snapshot.encoded_len() > MAX_BYTES {
        return Err(BuildError::Invalid);
    }
    let mut records = BTreeMap::new();
    for record in &snapshot.records {
        let id = reference(record)?.id;
        clock(record)?;
        if records.insert(id, record).is_some() {
            return Err(BuildError::Invalid);
        }
    }
    Ok(records)
}
/// Inputs are owned authorized captures. Caller reserves their decoded memory
/// before constructing them and validates the result against its live reducer.
pub fn build(
    previous: &p::Snapshot,
    mut target: p::Snapshot,
    retired: &BTreeSet<u64>,
) -> Result<Candidate, BuildError> {
    let prior = inventory(previous)?;
    let current = inventory(&target)?;
    if current.keys().any(|id| retired.contains(id)) {
        return Err(BuildError::Invalid);
    }
    let source_clocks = current
        .iter()
        .map(|(id, r)| Ok((*id, clock(r)?)))
        .collect::<Result<BTreeMap<_, _>, BuildError>>()?;
    drop(current);
    target.revision = next(previous.revision)?;
    let mut mutations = Vec::new();
    let mut seen = BTreeSet::new();
    for record in &mut target.records {
        let reference = reference(record)?;
        let old = prior.get(&reference.id).copied();
        if old.is_some_and(|r| self::reference(r).is_ok_and(|r| r.kind != reference.kind)) {
            return Err(BuildError::Invalid);
        }
        normalize(record, old)?;
        seen.insert(reference.id);
        if old != Some(record) {
            mutations.push(p::Mutation {
                mutation: Some(p::mutation::Mutation::Put(record.clone())),
            });
        }
    }
    for (id, record) in prior {
        if !seen.contains(&id) {
            mutations.push(p::Mutation {
                mutation: Some(p::mutation::Mutation::Delete(reference(record)?)),
            });
        }
    }
    target.records.sort_by_key(|r| reference(r).unwrap().id);
    target.required_resources.sort();
    target.required_resources.dedup();
    target.required_features.sort();
    target.required_features.dedup();
    let transaction = p::Transaction {
        base_revision: previous.revision,
        new_revision: target.revision,
        mutations,
        required_resources: target.required_resources.clone(),
        required_features: target.required_features.clone(),
    };
    if transaction.encoded_len() > MAX_BYTES {
        return Err(BuildError::Invalid);
    }
    Ok(Candidate {
        snapshot: target,
        transaction,
        source_clocks,
    })
}
impl Candidate {
    /// Both reducers fork the identical committed predecessor, including its
    /// permanent retirements and resource requirements. No fresh snapshot is
    /// used to reconstruct a sequence's retirement history.
    pub fn validate(
        &self,
        previous: &Replica,
        available: &BTreeSet<[u8; 32]>,
        limits: &Limits,
    ) -> Result<(), BuildError> {
        let mut delta = previous.fork(limits.clone())?;
        let mut snapshot = previous.fork(limits.clone())?;
        let delta_outcome =
            delta.apply_transaction(&self.transaction.encode_to_vec(), available)?;
        let snapshot_outcome =
            snapshot.apply_snapshot(&self.snapshot.encode_to_vec(), available)?;
        if !matches!(delta_outcome, ApplyOutcome::Applied { .. })
            || !matches!(snapshot_outcome, ApplyOutcome::Applied { .. })
            || delta.state() != snapshot.state()
            || delta.resource_refs() != snapshot.resource_refs()
        {
            return Err(BuildError::Divergence);
        }
        Ok(())
    }
}
