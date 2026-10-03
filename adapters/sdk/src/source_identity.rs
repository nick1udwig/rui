//! Broker-owned app identity namespace across disposable source processes.
//! The caller reserves the fixed 4MiB namespace allowance before construction.
use crate::gtk_host::GtkHostError;
use rui_protocol_core::wire::proto as p;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const MAX_LIVE_IDS: usize = 4096;
const MAX_ISSUED_IDS: usize = 8192;

#[derive(Clone)]
pub(crate) struct SourceIdentities {
    generation: u64,
    session: u64,
    next: u64,
    to_public: BTreeMap<u64, u64>,
    to_native: BTreeMap<u64, u64>,
    issued: BTreeSet<u64>,
    focus: (Option<u64>, Option<u64>),
    native_focus_generation: Option<u64>,
    pub(crate) focus_generation: u64,
}
impl SourceIdentities {
    pub(crate) fn new() -> Self {
        Self {
            generation: 1,
            session: 0,
            next: 1,
            to_public: BTreeMap::new(),
            to_native: BTreeMap::new(),
            issued: BTreeSet::new(),
            focus: (None, None),
            native_focus_generation: None,
            focus_generation: 1,
        }
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub(crate) fn private_id(&self, public: u64) -> Result<u64, GtkHostError> {
        self.to_native
            .get(&public)
            .copied()
            .ok_or(GtkHostError::Unsupported)
    }
    pub(crate) fn public_id(&self, private: u64) -> Result<u64, GtkHostError> {
        self.to_public
            .get(&private)
            .copied()
            .ok_or(GtkHostError::Unsupported)
    }
    pub(crate) fn reset(&mut self) -> Result<(), GtkHostError> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(GtkHostError::Unsupported)?;
        self.to_public.clear();
        self.to_native.clear();
        self.native_focus_generation = None;
        Ok(())
    }
    pub(crate) fn unavailable(
        &mut self,
        revision: u64,
        canvas: p::Rect,
    ) -> Result<p::Snapshot, GtkHostError> {
        if self.focus != (None, None) {
            self.focus_generation = self
                .focus_generation
                .checked_add(1)
                .ok_or(GtkHostError::Unsupported)?;
        }
        self.focus = (None, None);
        self.to_public.clear();
        self.to_native.clear();
        self.native_focus_generation = None;
        Ok(p::Snapshot {
            revision,
            records: vec![p::Record {
                record: Some(p::record::Record::Session(p::SessionState {
                    id: self.session,
                    version: revision,
                    canvas: Some(canvas),
                    cursor: None,
                    focus: None,
                    focus_surface: None,
                    focus_generation: self.focus_generation,
                })),
            }],
            required_resources: vec![],
            required_features: vec![],
        })
    }
    pub(crate) fn project(
        &mut self,
        snapshot: &mut p::Snapshot,
        offset: u64,
    ) -> Result<(), GtkHostError> {
        // The 4MiB namespace allowance covers both bounded sparse map/set
        // copies and the temporary ID set. Failed projections cannot leave
        // partially updated dispatch authority; the owned snapshot is discarded.
        let mut staged = self.clone();
        staged.project_inner(snapshot, offset)?;
        *self = staged;
        Ok(())
    }
    fn project_inner(
        &mut self,
        snapshot: &mut p::Snapshot,
        offset: u64,
    ) -> Result<(), GtkHostError> {
        if snapshot.records.len() > MAX_LIVE_IDS {
            return Err(GtkHostError::Unsupported);
        }
        let sessions: Vec<_> = snapshot
            .records
            .iter()
            .filter_map(|r| match r.record.as_ref() {
                Some(p::record::Record::Session(s)) => Some(s),
                _ => None,
            })
            .collect();
        if sessions.len() != 1 {
            return Err(GtkHostError::Unsupported);
        }
        let raw_session = sessions[0].id;
        if self.session == 0 {
            if raw_session == 0 || raw_session > MAX_ISSUED_IDS as u64 {
                return Err(GtkHostError::Unsupported);
            }
            self.session = raw_session;
            self.issued.insert(raw_session);
        }
        let private_ids: BTreeSet<_> = snapshot
            .records
            .iter()
            .map(record_id)
            .collect::<Result<_, _>>()?;
        self.to_public.retain(|native, public| {
            let keep = private_ids.contains(native) && *native != raw_session;
            if !keep {
                self.to_native.remove(public);
            }
            keep
        });
        for id in &private_ids {
            if *id == raw_session || self.to_public.contains_key(id) {
                continue;
            }
            if self.issued.len() >= MAX_ISSUED_IDS {
                return Err(GtkHostError::Unsupported);
            }
            let public = if self.generation == 1 {
                if *id == 0 || *id > MAX_ISSUED_IDS as u64 || self.issued.contains(id) {
                    return Err(GtkHostError::Unsupported);
                }
                *id
            } else {
                while self.issued.contains(&self.next) {
                    self.next = self.next.checked_add(1).ok_or(GtkHostError::Unsupported)?;
                }
                let id = self.next;
                self.next = self.next.checked_add(1).ok_or(GtkHostError::Unsupported)?;
                id
            };
            self.issued.insert(public);
            self.next = self
                .next
                .max(public.checked_add(1).ok_or(GtkHostError::Unsupported)?);
            self.to_public.insert(*id, public);
            self.to_native.insert(public, *id);
        }
        snapshot.revision = snapshot
            .revision
            .checked_add(offset)
            .ok_or(GtkHostError::Unsupported)?;
        let remap = |reference: &mut p::ObjectRef| -> Result<(), GtkHostError> {
            reference.id = if reference.id == raw_session && reference.kind == 6 {
                self.session
            } else {
                self.public_id(reference.id)?
            };
            Ok(())
        };
        let optional = |reference: &mut Option<p::ObjectRef>| -> Result<(), GtkHostError> {
            if let Some(reference) = reference {
                remap(reference)?;
            }
            Ok(())
        };
        let mut native_generation = None;
        let mut focus = (None, None);
        for raw in &mut snapshot.records {
            use p::record::Record;
            match raw.record.as_mut().ok_or(GtkHostError::Unsupported)? {
                Record::Surface(s) => {
                    s.id = self.public_id(s.id)?;
                    for r in [
                        &mut s.parent_surface,
                        &mut s.modal_for,
                        &mut s.scene_root,
                        &mut s.semantic_root,
                    ] {
                        optional(r)?;
                    }
                }
                Record::Scene(s) => {
                    s.id = self.public_id(s.id)?;
                    for r in &mut s.ordered_children {
                        remap(r)?;
                    }
                }
                Record::Semantic(s) => {
                    s.id = self.public_id(s.id)?;
                    for r in s
                        .ordered_children
                        .iter_mut()
                        .chain(s.visual_bindings.iter_mut())
                    {
                        remap(r)?;
                    }
                    for r in [&mut s.text_model, &mut s.semantic_group] {
                        optional(r)?;
                    }
                    for relation in &mut s.relations {
                        for r in &mut relation.targets {
                            remap(r)?;
                        }
                    }
                    if let Some(c) = &mut s.collection_model {
                        for r in c.items.iter_mut().chain(c.selected_items.iter_mut()) {
                            remap(r)?;
                        }
                    }
                }
                Record::Text(s) => {
                    s.id = self.public_id(s.id)?;
                }
                Record::Binding(s) => {
                    s.id = self.public_id(s.id)?;
                    optional(&mut s.surface)?;
                    for r in s.semantic_nodes.iter_mut().chain(s.scene_nodes.iter_mut()) {
                        remap(r)?;
                    }
                }
                Record::Session(s) => {
                    s.id = self.session;
                    s.version = snapshot.revision;
                    optional(&mut s.focus)?;
                    optional(&mut s.focus_surface)?;
                    native_generation = Some(s.focus_generation);
                    focus = (
                        s.focus.as_ref().map(|r| r.id),
                        s.focus_surface.as_ref().map(|r| r.id),
                    );
                }
            }
        }
        let native_generation = native_generation.ok_or(GtkHostError::Unsupported)?;
        if let Some(old) = self.native_focus_generation {
            if native_generation < old {
                return Err(GtkHostError::Unsupported);
            }
            self.focus_generation = self
                .focus_generation
                .checked_add(native_generation - old)
                .ok_or(GtkHostError::Unsupported)?;
        } else if self.generation == 1 {
            self.focus_generation = native_generation;
        } else if focus != self.focus {
            self.focus_generation = self
                .focus_generation
                .checked_add(1)
                .ok_or(GtkHostError::Unsupported)?;
        }
        self.focus = focus;
        self.native_focus_generation = Some(native_generation);
        for record in &mut snapshot.records {
            if let Some(p::record::Record::Session(session)) = record.record.as_mut() {
                session.focus_generation = self.focus_generation;
            }
        }
        Ok(())
    }
}
fn record_id(record: &p::Record) -> Result<u64, GtkHostError> {
    use p::record::Record;
    Ok(
        match record.record.as_ref().ok_or(GtkHostError::Unsupported)? {
            Record::Surface(s) => s.id,
            Record::Scene(s) => s.id,
            Record::Semantic(s) => s.id,
            Record::Text(s) => s.id,
            Record::Binding(s) => s.id,
            Record::Session(s) => s.id,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CaptureArtifact;
    use prost::Message;
    use rui_protocol_core::model::{ApplyOutcome, Limits, Replica};
    use std::path::PathBuf;

    #[test]
    fn verified_multisurface_graph_survives_remapping_without_retired_reuse() {
        let root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gtk/whole-golden");
        let artifact = CaptureArtifact::read(root.join("whole-0001.json")).unwrap();
        let mut verified = artifact.receiver().unwrap();
        artifact.apply_trusted_fixture(&mut verified).unwrap();
        let resources = verified.state().unwrap().resource_refs().clone();
        let envelope = p::Envelope::decode(
            std::fs::read(root.join("whole-0001.bin"))
                .unwrap()
                .as_slice(),
        )
        .unwrap();
        let original = p::Snapshot::decode(envelope.payload.as_slice()).unwrap();
        let mut ids = SourceIdentities::new();
        let mut replica = Replica::new(Limits::default());
        let mut first = original.clone();
        ids.project(&mut first, 0).unwrap();
        assert_eq!(
            replica
                .apply_snapshot(&first.encode_to_vec(), &resources)
                .unwrap(),
            ApplyOutcome::Applied { revision: 1 }
        );
        let old: BTreeSet<_> = replica
            .state()
            .records
            .keys()
            .copied()
            .filter(|id| *id != ids.session)
            .collect();
        let focus = ids.focus_generation;
        let canvas = first
            .records
            .iter()
            .find_map(|r| match &r.record {
                Some(p::record::Record::Session(s)) => s.canvas,
                _ => None,
            })
            .unwrap();
        let unavailable = ids.unavailable(2, canvas).unwrap();
        replica
            .apply_snapshot(&unavailable.encode_to_vec(), &BTreeSet::new())
            .unwrap();
        assert_eq!(replica.state().records.len(), 1);
        assert!(old.is_subset(&replica.state().retired));
        ids.reset().unwrap();
        let mut second = original;
        // The replacement's own Session ID can change or collide with an old
        // app ID. It never replaces the surviving broker Session namespace.
        for record in &mut second.records {
            if let Some(p::record::Record::Session(s)) = &mut record.record {
                s.id = 10000;
            }
        }
        ids.project(&mut second, 2).unwrap();
        replica
            .apply_snapshot(&second.encode_to_vec(), &resources)
            .unwrap();
        assert_eq!(replica.state().revision, 3);
        assert!(old.is_disjoint(&replica.state().records.keys().copied().collect()));
        assert!(replica.state().records.contains_key(&ids.session));
        assert!(ids.focus_generation > focus);
        for (private, public) in &ids.to_public {
            assert_eq!(ids.private_id(*public).unwrap(), *private);
            assert_ne!(*public, ids.session);
        }
        verified.close();
        assert_eq!(verified.memory_used(), 0);
    }

    fn bounded_snapshot(count: usize) -> p::Snapshot {
        let mut records = vec![p::Record {
            record: Some(p::record::Record::Session(p::SessionState {
                id: 1,
                version: 1,
                focus_generation: 1,
                ..Default::default()
            })),
        }];
        records.extend((2..=count as u64).map(|id| p::Record {
            record: Some(p::record::Record::Text(p::TextModel {
                id,
                version: 1,
                ..Default::default()
            })),
        }));
        p::Snapshot {
            revision: 1,
            records,
            ..Default::default()
        }
    }

    #[test]
    fn live_and_lifetime_identity_limits_fail_without_partial_authority() {
        let mut ids = SourceIdentities::new();
        assert!(
            ids.project(&mut bounded_snapshot(MAX_LIVE_IDS + 1), 0)
                .is_err()
        );
        assert_eq!(ids.session, 0);
        assert!(ids.issued.is_empty());
        ids.project(&mut bounded_snapshot(MAX_LIVE_IDS), 0).unwrap();
        ids.reset().unwrap();
        ids.project(&mut bounded_snapshot(MAX_LIVE_IDS), 2).unwrap();
        assert_eq!(ids.issued.len(), 8191);
        ids.reset().unwrap();
        assert!(ids.project(&mut bounded_snapshot(3), 4).is_err());
        assert_eq!(ids.issued.len(), 8191);
        assert!(ids.to_public.is_empty() && ids.to_native.is_empty());
        // A smaller valid capture can consume the remaining identity slot.
        ids.project(&mut bounded_snapshot(2), 4).unwrap();
        assert_eq!(ids.issued.len(), MAX_ISSUED_IDS);
    }
}
