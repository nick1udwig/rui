//! A cancellation belongs to the authority that admitted its native IM owner.
//! It cannot acquire a later grant merely because the UI has changed.
use super::UiState;
use crate::{
    interaction::{CommandBinding, EditingContext},
    physical_dispatch::PhysicalControl,
};

pub fn owns_binding(owner: EditingContext, binding: &CommandBinding) -> bool {
    binding.lease != [0; 16]
        && binding.generation > 0
        && (owner.epoch, owner.view, owner.lease, owner.generation)
            == (
                binding.epoch,
                binding.view,
                binding.lease,
                binding.generation,
            )
}
fn same_authority(left: EditingContext, right: EditingContext) -> bool {
    (left.epoch, left.view, left.lease, left.generation)
        == (right.epoch, right.view, right.lease, right.generation)
}
#[derive(Clone, Copy, Default)]
pub struct CleanupOwner(Option<(EditingContext, PhysicalControl)>);
impl CleanupOwner {
    /// Capture the token before the ordinary FIFO can fail. No allocation and
    /// no inference from the current grant at cancellation time are allowed.
    pub fn remember(&mut self, owner: EditingContext, ui: UiState) -> bool {
        if let Some((editing, control)) = ui.editing.zip(ui.composition_control) {
            if same_authority(owner, editing) {
                self.0 = Some((owner, control));
                return true;
            }
        }
        false
    }
    pub fn stop_for(&self, owner: Option<EditingContext>) -> Option<PhysicalControl> {
        let (old, control) = self.0?;
        owner
            .filter(|owner| same_authority(old, *owner))
            .map(|_| control)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use rui_protocol_core::model::TextRange;
    fn owner() -> EditingContext {
        EditingContext {
            epoch: [1; 16],
            view: [2; 16],
            lease: [3; 16],
            generation: 4,
            revision: 1,
            target: 20,
            target_version: 1,
            text: 40,
            text_record_version: 1,
            text_version: 1,
            focus_generation: 1,
            selection: TextRange {
                anchor: 0,
                focus: 0,
            },
        }
    }
    #[test]
    fn old_queued_stages_never_acquire_or_stop_a_new_grant() {
        let old = owner();
        let mut current = old;
        current.lease = [9; 16];
        current.generation += 1;
        let binding = CommandBinding {
            epoch: current.epoch,
            view: current.view,
            client: [8; 16],
            lease: current.lease,
            generation: current.generation,
            sequence: 1,
        };
        assert!(!owns_binding(old, &binding));
        assert!(owns_binding(current, &binding));
        for case in 0..4 {
            let mut stale = current;
            match case {
                0 => stale.epoch = [7; 16],
                1 => stale.view = [7; 16],
                2 => stale.lease = [7; 16],
                3 => stale.generation += 1,
                _ => unreachable!(),
            }
            assert!(!owns_binding(stale, &binding));
        }
        let old_token = PhysicalControl {
            connection_generation: 1,
            grant_serial: 1,
        };
        let new_token = PhysicalControl {
            connection_generation: 1,
            grant_serial: 2,
        };
        let mut cleanup = CleanupOwner::default();
        assert!(cleanup.remember(
            old,
            UiState {
                editing: Some(old),
                composition_control: Some(old_token),
                ..Default::default()
            }
        ));
        // UI may already display the new grant, but only the remembered old
        // token can clean up an old Cancel that failed during native reset.
        let new_ui = UiState {
            editing: Some(current),
            composition_control: Some(new_token),
            ..Default::default()
        };
        assert_eq!(cleanup.stop_for(Some(old)), Some(old_token));
        assert_eq!(cleanup.stop_for(Some(current)), None);
        assert!(!cleanup.remember(old, new_ui));
        assert_eq!(cleanup.stop_for(Some(old)), Some(old_token));
        assert!(cleanup.remember(current, new_ui));
        assert_eq!(cleanup.stop_for(Some(old)), None);
        assert_eq!(cleanup.stop_for(Some(current)), Some(new_token));
        assert_eq!(cleanup.stop_for(None), None);
    }
}
