//! View clocks describe published full replacements. Native clocks continue to
//! guard routing/focus privately in the single authoritative Topology.
use crate::{topology::Projection, ShellError};
use rui_protocol_core::{
    model::{Limits, State},
    resources::{MemoryBudget, MemoryReservation},
    wire::proto as pb,
};
use std::collections::BTreeSet;

pub struct ProjectedCapture {
    pub(super) state: State,
    pub(super) records: Vec<pb::Record>,
    source_revision: u64,
    _memory: MemoryReservation,
    _view_memory: Option<rui_protocol_core::session::ViewMemoryReservation>,
}
impl ProjectedCapture {
    pub fn state(&self) -> &State {
        &self.state
    }
    pub fn records(&self) -> &[pb::Record] {
        &self.records
    }
    pub fn source_revision(&self) -> u64 {
        self.source_revision
    }
    pub(super) fn charge_view(
        &mut self,
        memory: rui_protocol_core::session::ViewMemoryReservation,
    ) {
        self._view_memory = Some(memory);
    }
}
pub(super) fn copy(
    capture: &ProjectedCapture,
    budget: &MemoryBudget,
) -> Result<ProjectedCapture, ShellError> {
    let mut memory = budget.reserve(2 << 20)?;
    let state = capture.state.clone();
    let records = capture.records.clone();
    memory.resize(state.estimated_bytes() + records.len() * 4096 + 2048)?;
    Ok(ProjectedCapture {
        state,
        records,
        source_revision: capture.source_revision,
        _memory: memory,
        _view_memory: None,
    })
}
#[cfg(test)]
fn next(
    source: &Projection,
    old: Option<&ProjectedCapture>,
    budget: &MemoryBudget,
) -> Result<(ProjectedCapture, Vec<pb::Mutation>), ShellError> {
    next_with_limits(source, old, budget, &Limits::default())
}
pub(super) fn next_with_limits(
    source: &Projection,
    old: Option<&ProjectedCapture>,
    budget: &MemoryBudget,
    limits: &Limits,
) -> Result<(ProjectedCapture, Vec<pb::Mutation>), ShellError> {
    if source.records().len() > 97 || source.records().len() > limits.max_records {
        return Err(ShellError::Limit);
    }
    // Admission precedes every clone/map/state conversion. The owned native
    // source vocabulary is <=16 windows, each with six bounded records.
    let mut memory = budget.reserve(2 << 20)?;
    let mut records = source.records().to_vec();
    let prior =
        rui_protocol_core::wire::record_inventory(old.map_or(&[], |p| p.records.as_slice()))
            .map_err(|_| ShellError::Geometry)?;
    let current = rui_protocol_core::wire::record_inventory(&records)
        .map_err(|_| ShellError::Geometry)?
        .keys()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut mutations = Vec::with_capacity(records.len() + old.map_or(0, |p| p.records.len()));
    for record in &mut records {
        let (id, kind, _) = identity(record)?;
        if let Some(previous) = prior.get(&id).copied() {
            let (_, previous_kind, version) = identity(previous)?;
            if kind != previous_kind {
                return Err(ShellError::Geometry);
            }
            set_version(record, version)?;
            if let (
                Some(pb::record::Record::Session(current)),
                Some(pb::record::Record::Session(prior)),
            ) = (record.record.as_mut(), previous.record.as_ref())
            {
                current.focus_generation = if current.focus == prior.focus
                    && current.focus_surface == prior.focus_surface
                {
                    prior.focus_generation
                } else {
                    prior
                        .focus_generation
                        .checked_add(1)
                        .ok_or(ShellError::Limit)?
                };
            }
            if record != previous {
                set_version(record, version.checked_add(1).ok_or(ShellError::Limit)?)?;
                mutations.push(pb::Mutation {
                    mutation: Some(pb::mutation::Mutation::Put(record.clone())),
                });
            }
        } else if old.is_some() {
            if old.is_some_and(|p| p.state.retired.contains(&id)) {
                return Err(ShellError::Geometry);
            }
            set_version(record, 1)?;
            mutations.push(pb::Mutation {
                mutation: Some(pb::mutation::Mutation::Put(record.clone())),
            });
        }
    }
    let mut retired = old.map_or_else(BTreeSet::new, |p| p.state.retired.clone());
    if let Some(old) = old {
        for previous in &old.records {
            let (id, kind, _) = identity(previous)?;
            if !current.contains(&id) {
                retired.insert(id);
                mutations.push(pb::Mutation {
                    mutation: Some(pb::mutation::Mutation::Delete(pb::ObjectRef { kind, id })),
                });
            }
        }
    }
    let revision = match old {
        None => 1,
        Some(old) if mutations.is_empty() => old.state.revision,
        Some(old) => old.state.revision.checked_add(1).ok_or(ShellError::Limit)?,
    };
    let mut state = State::from_source_records(revision, records.clone(), limits)?;
    state.retired = retired;
    if state.retired.len() > limits.max_retired || state.estimated_bytes() > limits.max_memory_bytes
    {
        return Err(ShellError::Limit);
    }
    memory.resize(
        state.estimated_bytes() + records.len() * 4096 + mutations.capacity() * 4096 + 2048,
    )?;
    Ok((
        ProjectedCapture {
            state,
            records,
            source_revision: source.state().revision,
            _memory: memory,
            _view_memory: None,
        },
        mutations,
    ))
}
fn identity(record: &pb::Record) -> Result<(u64, i32, u64), ShellError> {
    let (id, kind, version) = record.identity().ok_or(ShellError::Geometry)?;
    if kind == pb::ObjectKind::Text {
        return Err(ShellError::Native("native shell has no text-model profile"));
    }
    Ok((id, kind as i32, version))
}
fn set_version(record: &mut pb::Record, version: u64) -> Result<(), ShellError> {
    identity(record)?;
    *record.version_mut().unwrap() = version;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        topology::{Topology, WindowSpec},
        Framebuffer, InjectionScope, RegionGrant,
    };
    use prost::Message;
    use rui_protocol_core::model::{
        Affine, Color, Point, Record, Rect, Replica, Size, SurfaceRole,
    };
    fn source() -> (Topology, MemoryBudget, u64, u64) {
        let budget = MemoryBudget::new(128 << 20);
        let grant = RegionGrant::new(
            InjectionScope::IsolatedWholeSession,
            Rect {
                x: 0.,
                y: 0.,
                width: 200.,
                height: 120.,
            },
            Rect {
                x: 40.,
                y: 80.,
                width: 600.,
                height: 240.,
            },
            Framebuffer {
                width: 1000,
                height: 700,
            },
            2.,
        )
        .unwrap();
        let mut topology = Topology::new(grant, &budget).unwrap();
        let spec = |x| WindowSpec {
            role: SurfaceRole::Toplevel,
            parent: None,
            modal_for: None,
            desktop_rect: Rect {
                x,
                y: 10.,
                width: 80.,
                height: 80.,
            },
            content_size: Size {
                width: 80.,
                height: 80.,
            },
            transform: Affine {
                xx: 1.,
                yy: 1.,
                xy: 0.,
                yx: 0.,
                tx: 0.,
                ty: 0.,
            },
            stacking_order: 0,
            color: Color {
                red: 0.,
                green: 0.,
                blue: 1.,
                alpha: 1.,
            },
        };
        let a = topology.open(spec(0.)).unwrap();
        let b = topology.open(spec(100.)).unwrap();
        topology.focus(a).unwrap();
        (topology, budget, a, b)
    }
    fn replica(capture: &ProjectedCapture) -> Replica {
        let mut replica = Replica::new(Limits::default());
        replica
            .apply_snapshot(
                &pb::Snapshot {
                    revision: capture.state.revision,
                    records: capture.records.clone(),
                    ..Default::default()
                }
                .encode_to_vec(),
                &BTreeSet::new(),
            )
            .unwrap();
        assert_eq!(replica.state(), capture.state());
        replica
    }
    fn delta(replica: &mut Replica, capture: &ProjectedCapture, mutations: Vec<pb::Mutation>) {
        let transaction = pb::Transaction {
            base_revision: replica.state().revision,
            new_revision: capture.state.revision,
            mutations,
            ..Default::default()
        };
        replica
            .apply_transaction(&transaction.encode_to_vec(), &BTreeSet::new())
            .unwrap();
        assert_eq!(replica.state(), capture.state());
    }
    #[test]
    fn multiple_native_effects_coalesce_to_one_legal_replacement_and_exact_reference() {
        let (mut topology, budget, a, _) = source();
        let (old, _) = next(&topology.snapshot().unwrap(), None, &budget).unwrap();
        let mut replica = replica(&old);
        topology.source_activity(a, "a").unwrap();
        topology.source_activity(a, "b").unwrap();
        topology.source_activity(a, "").unwrap();
        let raw = topology.snapshot().unwrap();
        let (new, mutations) = next(&raw, Some(&old), &budget).unwrap();
        assert!(new.source_revision() > old.source_revision() + 1);
        assert_eq!(new.state.revision, old.state.revision + 1);
        for mutation in &mutations {
            let Some(pb::mutation::Mutation::Put(record)) = &mutation.mutation else {
                panic!()
            };
            let (id, _, version) = identity(record).unwrap();
            assert_eq!(
                version,
                identity(
                    old.records
                        .iter()
                        .find(|r| identity(r).unwrap().0 == id)
                        .unwrap()
                )
                .unwrap()
                .2 + 1
            );
        }
        let Record::Surface(projected) = &new.state.records[&a] else {
            panic!()
        };
        let Record::Surface(native) = &raw.state().records[&a] else {
            panic!()
        };
        assert_eq!(projected.routing_token, native.routing_token);
        assert_eq!(
            projected.source_capture_tokens,
            native.source_capture_tokens
        );
        assert_eq!(projected.desktop_rect, native.desktop_rect);
        delta(&mut replica, &new, mutations);
        assert_eq!(copy(&new, &budget).unwrap().state(), replica.state());
    }
    #[test]
    fn focus_aba_preserves_view_clock_but_native_routing_provenance_changes() {
        let (mut topology, budget, a, b) = source();
        let (old, _) = next(&topology.snapshot().unwrap(), None, &budget).unwrap();
        let mut replica = replica(&old);
        topology.focus(b).unwrap();
        topology.focus(a).unwrap();
        let raw = topology.snapshot().unwrap();
        let (new, mutations) = next(&raw, Some(&old), &budget).unwrap();
        let Record::Session(old_session) = &old.state.records[&1] else {
            panic!()
        };
        let Record::Session(new_session) = &new.state.records[&1] else {
            panic!()
        };
        let Record::Session(native_session) = &raw.state().records[&1] else {
            panic!()
        };
        assert_eq!(new_session.focus_generation, old_session.focus_generation);
        assert!(native_session.focus_generation > new_session.focus_generation);
        let Record::Surface(before) = &old.state.records[&a] else {
            panic!()
        };
        let Record::Surface(after) = &new.state.records[&a] else {
            panic!()
        };
        assert_ne!(after.routing_token, before.routing_token);
        assert_eq!(
            after.routing_token,
            match &raw.state().records[&a] {
                Record::Surface(s) => s.routing_token.clone(),
                _ => panic!(),
            }
        );
        delta(&mut replica, &new, mutations);
    }
    #[test]
    fn unchanged_final_cursor_fields_do_not_invent_view_mutations() {
        let (mut topology, budget, _, _) = source();
        let (old, _) = next(&topology.snapshot().unwrap(), None, &budget).unwrap();
        topology.source_pointer(Point { x: 100., y: 100. }).unwrap();
        topology.source_pointer(Point { x: 40., y: 80. }).unwrap();
        let (new, mutations) = next(&topology.snapshot().unwrap(), Some(&old), &budget).unwrap();
        assert!(mutations.is_empty());
        assert_eq!(new.state(), old.state());
        assert!(new.source_revision() > old.source_revision());
    }
}
