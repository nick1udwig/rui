//! Main-thread capture of granted physical input. Queue pressure never drops a
//! possible held press silently: its original grant is stopped on a separate
//! priority channel, independent of the normal bounded command FIFO.
use super::{physical_native, Stamp, UiCommand, UiState};
use crate::{
    physical_dispatch::{CapturedPhysicalInput, PhysicalControl, PresentedRouting},
    physical_input::PhysicalKeyCapture,
};
use gtk4::{gdk, glib};
use rui_protocol_core::{
    input::{Event, TimedEvent},
    resources::MemoryBudget,
};
use std::{cell::RefCell, rc::Rc, sync::Arc};
use tokio::sync::{mpsc, watch};

struct State {
    keys: Option<PhysicalKeyCapture>,
    buttons: u32,
    pointer_control: Option<PhysicalControl>,
    pointer_owner: Option<crate::physical_input::PhysicalKeyOwner>,
    stopped: Option<PhysicalControl>,
}
#[derive(Clone)]
pub struct PhysicalCallbacks {
    state: Rc<RefCell<State>>,
    commands: mpsc::Sender<UiCommand>,
    priority: watch::Sender<Option<PhysicalControl>>,
}
impl PhysicalCallbacks {
    pub fn new(
        commands: mpsc::Sender<UiCommand>,
        priority: watch::Sender<Option<PhysicalControl>>,
    ) -> Self {
        Self {
            state: Rc::new(RefCell::new(State {
                keys: None,
                buttons: 0,
                pointer_control: None,
                pointer_owner: None,
                stopped: None,
            })),
            commands,
            priority,
        }
    }
    pub fn stop(&self, control: PhysicalControl) {
        let mut state = self.state.borrow_mut();
        state.stopped = Some(control);
        if let Some(keys) = &mut state.keys {
            keys.clear();
        }
        state.buttons = 0;
        state.pointer_control = None;
        state.pointer_owner = None;
        self.priority.send_replace(Some(control));
    }
    pub fn stop_current(&self, ui: UiState) {
        let old = {
            let state = self.state.borrow();
            state
                .keys
                .as_ref()
                .and_then(PhysicalKeyCapture::owner)
                .map(|owner| PhysicalControl {
                    connection_generation: owner.connection_generation,
                    grant_serial: owner.grant_serial,
                })
                .or(state.pointer_control)
                .or(ui.physical_control)
        };
        if let Some(control) = old {
            self.stop(control);
        }
    }
    pub fn reconcile(&self, ui: UiState) {
        let invalid = {
            let state = self.state.borrow();
            let key = state.keys.as_ref().and_then(PhysicalKeyCapture::owner);
            let old = key
                .map(|owner| PhysicalControl {
                    connection_generation: owner.connection_generation,
                    grant_serial: owner.grant_serial,
                })
                .or(state.pointer_control);
            old.filter(|control| {
                !ui.physical_ready
                    || ui.physical_control != Some(*control)
                    || key.is_some_and(|owner| ui.physical_keyboard != Some(owner))
            })
        };
        if let Some(old) = invalid {
            self.stop(old);
        }
    }
    pub fn replace_budget(&self) {
        let old = {
            let state = self.state.borrow();
            state
                .keys
                .as_ref()
                .and_then(PhysicalKeyCapture::owner)
                .map(|owner| PhysicalControl {
                    connection_generation: owner.connection_generation,
                    grant_serial: owner.grant_serial,
                })
                .or(state.pointer_control)
        };
        if let Some(old) = old {
            self.stop(old);
        }
        // New view budgets do not retain local capture metadata charged to a
        // retired view. Already sent input retains its source cleanup gate.
        self.state.borrow_mut().keys = None;
    }
    fn ready(&self, ui: UiState) -> Option<PhysicalControl> {
        let control = ui.physical_control?;
        (ui.physical_ready && self.state.borrow().stopped != Some(control)).then_some(control)
    }
    fn enqueue(
        &self,
        control: PhysicalControl,
        captured: CapturedPhysicalInput,
    ) -> Result<(), &'static str> {
        if self
            .commands
            .try_send(UiCommand::Physical(captured))
            .is_err()
        {
            self.stop(control);
            return Err("Physical input queue is full or disconnected; control stopped. Request fresh control.");
        }
        Ok(())
    }
    pub fn key_press(
        &self,
        display: &gdk::Display,
        hardware: u32,
        ui: UiState,
        stamp: Option<Stamp>,
        routing: Option<Arc<PresentedRouting>>,
        budget: Option<&MemoryBudget>,
    ) -> Result<(), &'static str> {
        self.reconcile(ui);
        let control = self
            .ready(ui)
            .ok_or("Physical input is paused; request fresh control after the current frame.")?;
        let result = (|| {
            let stamp = stamp.filter(|stamp| ui.accepts(*stamp)).ok_or(())?;
            let routing = routing
                .filter(|r| r.revision() == stamp.revision)
                .ok_or(())?;
            let owner = routing.keyboard_owner(control).ok_or(())?;
            if ui.physical_keyboard != Some(owner) {
                return Err(());
            }
            let budget = budget.ok_or(())?;
            let code = physical_native::evdev_keycode(display, hardware, budget).ok_or(())?;
            let mut state = self.state.borrow_mut();
            if state.keys.is_none() {
                state.keys = Some(PhysicalKeyCapture::new(budget).map_err(|_| ())?);
            }
            let Some(key) = state
                .keys
                .as_mut()
                .ok_or(())?
                .press(owner, hardware, code)
                .map_err(|_| ())?
            else {
                return Ok(None);
            };
            CapturedPhysicalInput::key(routing, control, stamp.viewport, timestamp(), key, budget)
                .map(Some)
                .map_err(|_| ())
        })();
        match result {
            Ok(Some(captured)) => self.enqueue(control, captured),
            Ok(None) => Ok(()),
            Err(()) => {
                self.stop(control);
                Err("Physical key mapping, route or memory is unavailable; control stopped. Request fresh control.")
            }
        }
    }
    pub fn key_release(
        &self,
        hardware: u32,
        ui: UiState,
        stamp: Option<Stamp>,
        routing: Option<Arc<PresentedRouting>>,
        budget: Option<&MemoryBudget>,
    ) -> Result<(), &'static str> {
        self.reconcile(ui);
        let owner = self
            .state
            .borrow()
            .keys
            .as_ref()
            .and_then(PhysicalKeyCapture::owner);
        let Some(owner) = owner else {
            return Ok(());
        };
        let control = PhysicalControl {
            connection_generation: owner.connection_generation,
            grant_serial: owner.grant_serial,
        };
        let result = (|| {
            if self.ready(ui) != Some(control) {
                return Err(());
            }
            let stamp = stamp.filter(|stamp| ui.accepts(*stamp)).ok_or(())?;
            let routing = routing
                .filter(|r| r.revision() == stamp.revision)
                .ok_or(())?;
            if routing.keyboard_owner(control) != Some(owner) {
                return Err(());
            }
            let budget = budget.ok_or(())?;
            let mut state = self.state.borrow_mut();
            let Some(key) = state
                .keys
                .as_mut()
                .ok_or(())?
                .release(owner, hardware)
                .map_err(|_| ())?
            else {
                return Ok(None);
            };
            CapturedPhysicalInput::key(routing, control, stamp.viewport, timestamp(), key, budget)
                .map(Some)
                .map_err(|_| ())
        })();
        match result {
            Ok(Some(captured)) => self.enqueue(control, captured),
            Ok(None) => Ok(()),
            Err(()) => {
                self.stop(control);
                Err("Physical release context changed; control stopped. Request fresh control.")
            }
        }
    }
    pub fn pointer(
        &self,
        ui: UiState,
        stamp: Option<Stamp>,
        routing: Option<Arc<PresentedRouting>>,
        event: Event,
        budget: Option<&MemoryBudget>,
    ) -> Result<(), &'static str> {
        self.reconcile(ui);
        let Some(control) = self.ready(ui) else {
            return Err("Physical pointer input is paused; request fresh control.");
        };
        let result = (|| {
            let stamp = stamp.filter(|stamp| ui.accepts(*stamp)).ok_or(())?;
            let routing = routing
                .filter(|r| r.revision() == stamp.revision)
                .ok_or(())?;
            let budget = budget.ok_or(())?;
            let mut state = self.state.borrow_mut();
            if let Event::Button {
                code,
                pressed: false,
                ..
            } = event
            {
                let bit = code.checked_sub(1).filter(|bit| *bit < 32).ok_or(())?;
                if state.buttons & (1u32 << bit) == 0 {
                    return Ok(None);
                }
                if state.pointer_control != Some(control) {
                    return Err(());
                }
            }
            let captured = CapturedPhysicalInput::pointer(
                routing,
                control,
                stamp.viewport,
                TimedEvent {
                    timestamp_us: timestamp(),
                    event: event.clone(),
                },
                budget,
                state.pointer_owner,
            )
            .map_err(|_| ())?;
            if let Event::Button { code, pressed, .. } = event {
                let bit = code.checked_sub(1).filter(|bit| *bit < 32).ok_or(())?;
                let mask = 1u32 << bit;
                if pressed {
                    if state.buttons & mask != 0 {
                        return Err(());
                    }
                    state.buttons |= mask;
                    state.pointer_control = Some(control);
                    state.pointer_owner = Some(captured.captured_owner());
                } else {
                    state.buttons &= !mask;
                    if state.buttons == 0 {
                        state.pointer_control = None;
                        state.pointer_owner = None;
                    }
                }
            }
            Ok(Some(captured))
        })();
        match result {
            Ok(Some(captured)) => self.enqueue(control, captured),
            Ok(None) => Ok(()),
            Err(()) => {
                self.stop(control);
                Err("Physical pointer route, geometry or memory changed; control stopped. Request fresh control.")
            }
        }
    }
    pub fn pointer_leave(&self) {
        let control = self.state.borrow().pointer_control;
        if let Some(control) = control {
            self.stop(control);
        }
    }
}
fn timestamp() -> u64 {
    u64::try_from(glib::monotonic_time()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        physical_input::{PhysicalKey, PhysicalKeyOwner},
        Viewport,
    };
    fn owner() -> PhysicalKeyOwner {
        PhysicalKeyOwner {
            connection_generation: 2,
            grant_serial: 3,
            surface: 10,
            routing_token: [7; 16],
        }
    }
    fn ui() -> UiState {
        UiState {
            generation: 2,
            physical_control: Some(PhysicalControl {
                connection_generation: 2,
                grant_serial: 3,
            }),
            physical_keyboard: Some(owner()),
            physical_ready: true,
            ..UiState::default()
        }
    }
    #[test]
    fn queue_overflow_uses_independent_priority_and_the_old_grant_cannot_rearm() {
        let budget = MemoryBudget::new(32 * 1024);
        let (commands, mut fifo) = mpsc::channel(1);
        commands
            .try_send(UiCommand::Acquire)
            .unwrap_or_else(|_| panic!());
        let (priority, signal) = watch::channel(None);
        let capture = PhysicalCallbacks::new(commands, priority);
        let mut keys = PhysicalKeyCapture::new(&budget).unwrap();
        keys.press(owner(), 50, 21).unwrap();
        capture.state.borrow_mut().keys = Some(keys);
        let exact = CapturedPhysicalInput::key(
            PresentedRouting::capture(&super::super::physical_continuity::tests::state(), &budget)
                .unwrap(),
            ui().physical_control.unwrap(),
            Viewport::new(100, 100),
            1,
            PhysicalKey {
                code: 21,
                pressed: true,
            },
            &budget,
        )
        .unwrap();
        let scope = ui().physical_control.unwrap();
        assert!(capture.enqueue(scope, exact).is_err());
        assert_eq!(*signal.borrow(), Some(scope));
        assert!(matches!(fifo.try_recv(), Ok(UiCommand::Acquire)));
        assert_eq!(capture.state.borrow().keys.as_ref().unwrap().held_len(), 0);
        assert_eq!(
            capture.ready(ui()),
            None,
            "ordinary UI polling cannot rearm the stopped grant"
        );
        let mut fresh = ui();
        fresh.physical_control.as_mut().unwrap().grant_serial += 1;
        assert_eq!(capture.ready(fresh), fresh.physical_control);
        drop(capture);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn readiness_and_focus_loss_neutralize_original_owner_without_stopping_a_new_grant() {
        let budget = MemoryBudget::new(32 * 1024);
        let (commands, _fifo) = mpsc::channel(1);
        let (priority, signal) = watch::channel(None);
        let capture = PhysicalCallbacks::new(commands, priority);
        let mut keys = PhysicalKeyCapture::new(&budget).unwrap();
        keys.press(owner(), 50, 21).unwrap();
        capture.state.borrow_mut().keys = Some(keys);
        capture.reconcile(ui());
        assert_eq!(*signal.borrow(), None);
        let mut missing = ui();
        missing.physical_ready = false;
        capture.reconcile(missing);
        assert_eq!(*signal.borrow(), ui().physical_control);
        assert_eq!(capture.ready(ui()), None);
        let mut fresh = ui();
        fresh.physical_control.as_mut().unwrap().grant_serial += 1;
        fresh.physical_keyboard.as_mut().unwrap().grant_serial += 1;
        capture
            .state
            .borrow_mut()
            .keys
            .as_mut()
            .unwrap()
            .press(owner(), 50, 21)
            .unwrap();
        capture.reconcile(fresh);
        assert_eq!(
            *signal.borrow(),
            ui().physical_control,
            "delayed old capture stops only its original owner"
        );
        assert_eq!(capture.ready(fresh), fresh.physical_control);
        drop(capture);
        assert_eq!(budget.used(), 0);
    }
}
