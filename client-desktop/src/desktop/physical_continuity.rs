//! Conservative held-input ownership until an authenticated dispatch result.
//! A cancelled socket write can already have caused a native press, so its
//! obligation remains until the connection is closed and source cleanup runs.
use crate::physical_dispatch::{self, PhysicalControl};
use crate::physical_input::PhysicalKeyOwner;
use rui_protocol_core::{
    input::Event,
    model::State,
    resources::{MemoryBudget, MemoryReservation},
};

#[derive(Clone, Copy, Default)]
struct Slot {
    sequence: u64,
    held: bool,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rui_protocol_core::model::*;
    fn control() -> PhysicalControl {
        PhysicalControl {
            connection_generation: 2,
            grant_serial: 3,
        }
    }
    fn owner() -> PhysicalKeyOwner {
        PhysicalKeyOwner {
            connection_generation: 2,
            grant_serial: 3,
            surface: 10,
            routing_token: [7; 16],
        }
    }
    pub(crate) fn state() -> State {
        let surface = Surface {
            id: 10,
            version: 1,
            role: SurfaceRole::Toplevel,
            parent: None,
            desktop_rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            content_size: Size {
                width: 100.0,
                height: 100.0,
            },
            transform: Affine {
                xx: 1.0,
                yy: 1.0,
                xy: 0.0,
                yx: 0.0,
                tx: 0.0,
                ty: 0.0,
            },
            clip: None,
            opacity: 1.0,
            stacking_order: 0,
            mapped: true,
            focused: true,
            modal_for: None,
            scene_root: None,
            semantic_root: None,
            input_region: vec![],
            routing_token: vec![7; 16],
            visual_coverage: VisualCoverage::Complete,
            source_capture_tokens: vec!["commit-1".into()],
            semantic_coverage: SemanticCoverage::Opaque,
            capture_consistency: CaptureConsistency::Coherent,
            unsupported_features: vec![],
        };
        let session = SessionState {
            id: 20,
            version: 1,
            canvas: Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            cursor: None,
            focus: None,
            focus_surface: Some(ObjectRef {
                kind: ObjectKind::Surface,
                id: 10,
            }),
            focus_generation: 1,
        };
        State {
            revision: 1,
            records: [
                (10, Record::Surface(surface)),
                (20, Record::Session(session)),
            ]
            .into(),
            retired: Default::default(),
        }
    }
    #[test]
    fn ordinary_visual_effect_keeps_held_key_but_source_route_or_focus_change_requires_cleanup() {
        let budget = MemoryBudget::new(64 * 1024);
        let mut pending = Continuity::new(&budget).unwrap();
        pending
            .before_send(
                1,
                owner(),
                &Event::Key {
                    code: 21,
                    pressed: true,
                },
            )
            .unwrap();
        assert!(pending.held());
        let mut state = state();
        state.revision = 2;
        if let Record::Surface(surface) = state.records.get_mut(&10).unwrap() {
            surface.opacity = 0.5;
            surface.source_capture_tokens = vec!["commit-2".into()];
            surface.version = 2;
        }
        assert!(pending.valid(&state, control()));
        assert_eq!(pending.result(1, "Dispatched"), None);
        pending
            .before_send(
                2,
                owner(),
                &Event::Key {
                    code: 21,
                    pressed: false,
                },
            )
            .unwrap();
        assert!(pending.held(), "release write could still be cancelled");
        assert_eq!(pending.result(2, "Dispatched"), None);
        assert!(!pending.held());
        pending
            .before_send(
                3,
                owner(),
                &Event::Key {
                    code: 21,
                    pressed: true,
                },
            )
            .unwrap();
        if let Record::Surface(surface) = state.records.get_mut(&10).unwrap() {
            surface.focused = false;
        }
        assert!(!pending.valid(&state, control()));
        if let Record::Surface(surface) = state.records.get_mut(&10).unwrap() {
            surface.focused = true;
            surface.desktop_rect.x = 3.0;
            surface.routing_token = vec![10; 16];
        }
        assert!(!pending.valid(&state, control()));
        assert_eq!(pending.result(3, "OutcomeUnknown"), Some(control()));
        pending.clear();
        assert!(!pending.held());
        drop(pending);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn older_release_result_does_not_erase_a_later_press_and_tracking_is_bounded() {
        let budget = MemoryBudget::new(64 * 1024);
        let mut pending = Continuity::new(&budget).unwrap();
        pending
            .before_send(
                1,
                owner(),
                &Event::Key {
                    code: 21,
                    pressed: true,
                },
            )
            .unwrap();
        pending
            .before_send(
                2,
                owner(),
                &Event::Key {
                    code: 21,
                    pressed: false,
                },
            )
            .unwrap();
        pending
            .before_send(
                3,
                owner(),
                &Event::Key {
                    code: 21,
                    pressed: true,
                },
            )
            .unwrap();
        assert_eq!(pending.result(2, "Dispatched"), None);
        assert!(pending.held());
        assert_eq!(pending.result(1, "Dispatched"), None);
        for sequence in 4..35 {
            pending
                .before_send(sequence, owner(), &Event::Motion(Point { x: 1.0, y: 1.0 }))
                .unwrap();
        }
        assert!(pending
            .before_send(35, owner(), &Event::Motion(Point { x: 1.0, y: 1.0 }))
            .is_err());
        assert_eq!(pending.result(3, "Failed"), Some(control()));
        let mut fresh = control();
        fresh.grant_serial += 1;
        assert!(!pending.valid(&state(), fresh));
        drop(pending);
        assert_eq!(budget.used(), 0);
    }
}
#[derive(Clone, Copy)]
enum Change {
    Key(usize, bool),
    Button(usize, bool),
    None,
}
#[derive(Clone, Copy)]
struct Flight {
    sequence: u64,
    control: PhysicalControl,
    change: Change,
}

pub struct Continuity {
    keys: [Slot; 768],
    buttons: [Slot; 32],
    flights: [Option<Flight>; 32],
    key_owner: Option<PhysicalKeyOwner>,
    pointer_owner: Option<PhysicalKeyOwner>,
    _memory: MemoryReservation,
}
impl Continuity {
    pub fn new(budget: &MemoryBudget) -> Result<Self, ()> {
        let memory = budget
            .reserve(std::mem::size_of::<Self>() + 4096)
            .map_err(|_| ())?;
        Ok(Self {
            keys: [Slot::default(); 768],
            buttons: [Slot::default(); 32],
            flights: [None; 32],
            key_owner: None,
            pointer_owner: None,
            _memory: memory,
        })
    }
    pub fn pointer_owner(&self) -> Option<PhysicalKeyOwner> {
        self.pointer_owner
    }
    pub fn held(&self) -> bool {
        self.key_owner.is_some() || self.pointer_owner.is_some()
    }
    pub fn valid(&self, state: &State, current: PhysicalControl) -> bool {
        self.key_owner
            .is_none_or(|old| physical_dispatch::keyboard_owner(state, current) == Some(old))
            && self.pointer_owner.is_none_or(|old| {
                old.connection_generation == current.connection_generation
                    && old.grant_serial == current.grant_serial
                    && physical_dispatch::pointer_owner_is_current(state, old)
            })
    }
    /// Register a possible press before any socket work. Releases retain the
    /// conservative union until the matching dispatch result is received.
    pub fn before_send(
        &mut self,
        sequence: u64,
        owner: PhysicalKeyOwner,
        event: &Event,
    ) -> Result<(), ()> {
        let index = self.flights.iter().position(Option::is_none).ok_or(())?;
        let change = match *event {
            Event::Key { code, pressed } => {
                let key = usize::try_from(code).map_err(|_| ())?;
                if key == 0
                    || key >= self.keys.len()
                    || self.key_owner.is_some_and(|old| old != owner)
                {
                    return Err(());
                }
                let slot = &mut self.keys[key];
                if !pressed && !slot.held {
                    return Err(());
                }
                slot.sequence = sequence;
                slot.held = true;
                self.key_owner = Some(owner);
                Change::Key(key, pressed)
            }
            Event::Button { code, pressed, .. } => {
                let button = usize::try_from(code)
                    .map_err(|_| ())?
                    .checked_sub(1)
                    .filter(|n| *n < self.buttons.len())
                    .ok_or(())?;
                if self.pointer_owner.is_some_and(|old| old != owner) {
                    return Err(());
                }
                let slot = &mut self.buttons[button];
                if !pressed && !slot.held {
                    return Err(());
                }
                slot.sequence = sequence;
                slot.held = true;
                self.pointer_owner = Some(owner);
                Change::Button(button, pressed)
            }
            _ => Change::None,
        };
        self.flights[index] = Some(Flight {
            sequence,
            control: PhysicalControl {
                connection_generation: owner.connection_generation,
                grant_serial: owner.grant_serial,
            },
            change,
        });
        Ok(())
    }
    /// A rejected/unknown dispatch requires stopping that original grant.
    /// Delayed results are never applied to a newer local grant serial.
    pub fn result(&mut self, sequence: u64, status: &str) -> Option<PhysicalControl> {
        let index = self
            .flights
            .iter()
            .position(|f| f.is_some_and(|f| f.sequence == sequence))?;
        let flight = self.flights[index].take()?;
        if !matches!(status, "Dispatched" | "DISPATCHED" | "Applied" | "APPLIED") {
            return Some(flight.control);
        }
        match flight.change {
            Change::Key(key, false) if self.keys[key].sequence == sequence => {
                self.keys[key].held = false;
                if !self.keys.iter().any(|s| s.held) {
                    self.key_owner = None;
                }
            }
            Change::Button(button, false) if self.buttons[button].sequence == sequence => {
                self.buttons[button].held = false;
                if !self.buttons.iter().any(|s| s.held) {
                    self.pointer_owner = None;
                }
            }
            _ => {}
        }
        None
    }
    pub fn clear(&mut self) {
        self.keys.fill(Slot::default());
        self.buttons.fill(Slot::default());
        self.flights.fill(None);
        self.key_owner = None;
        self.pointer_owner = None;
    }
}
