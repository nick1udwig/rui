//! One bounded, lease-owned cooperating-source input context. No OS injection.
use rui_protocol_core::{
    commands::{
        CommandFamily, CompositionOperation, Id, RawInputCommand, Rejection, TextCompositionCommand,
    },
    input::{
        CompositionMachine, EditingContext, Event, InputError, InputLimits, InputRouter, Route,
        TextEdit, TimedEvent,
    },
    session::{ControlLease, PhysicalKey, SessionBroker, ViewMemoryReservation},
};

pub const FIXTURE_SEAT: &str = "fixture-seat";
pub const FIXTURE_DEVICE: &str = "fixture-pointer";
pub const MAX_RAW_SAMPLES: usize = 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct RawSample {
    pub command_sequence: u64,
    pub event_index: u16,
    pub event: TimedEvent,
}
pub(crate) struct SourceInput {
    pub lease: ControlLease,
    router: InputRouter,
    composition: CompositionMachine,
    trace: Vec<RawSample>,
    route_token: Option<Id>,
    _memory: ViewMemoryReservation,
}
impl SourceInput {
    pub fn memory_bytes(max_text: usize) -> usize {
        MAX_RAW_SAMPLES * 192 + 128 * 128 + max_text * 4 + 64 * 1024
    }
    pub fn new(lease: ControlLease, max_text: usize, memory: ViewMemoryReservation) -> Self {
        Self {
            lease,
            // The authored canvas has one drag owner. Mixed or simultaneous
            // pointer contacts fail preflight rather than replacing that owner.
            router: InputRouter::new(
                crate::SURFACE_ID,
                InputLimits {
                    max_held: 1,
                    ..InputLimits::default()
                },
            )
            .unwrap(),
            composition: CompositionMachine::new(128, max_text).unwrap(),
            trace: Vec::with_capacity(MAX_RAW_SAMPLES),
            route_token: None,
            _memory: memory,
        }
    }
    pub fn samples(&self) -> &[RawSample] {
        &self.trace
    }
    pub fn preedit(&self) -> Option<&str> {
        self.composition.preedit()
    }
    pub fn range(&self) -> Option<rui_protocol_core::input::ScalarRange> {
        self.composition.active_range()
    }
    pub fn source_token(&self) -> Option<Id> {
        self.route_token
    }
    /// Trusted source changes cancel transient state before publishing it.
    pub fn synchronize(
        &mut self,
        context: Option<EditingContext>,
        token: Id,
        sessions: &mut SessionBroker,
        now: u64,
    ) {
        if self.composition.set_context(context).is_some() {
            let _ = sessions.set_composition(&self.lease, None, now);
        }
        if self.route_token.is_some_and(|old| old != token) {
            if self.composition.cancel().is_some() {
                let _ = sessions.set_composition(&self.lease, None, now);
            }
            self.cleanup_router(sessions, now);
        }
        self.route_token = Some(token);
    }
    fn cleanup_router(&mut self, sessions: &mut SessionBroker, now: u64) {
        let held = self.router.drain_cleanup();
        for code in held.keys {
            let _ = sessions.confirm_key_released(
                &self.lease,
                &PhysicalKey {
                    namespace: "linux.evdev".into(),
                    code,
                },
                now,
            );
        }
        for button in held.buttons {
            let _ = sessions.confirm_button_released(&self.lease, button, now);
        }
        for touch in held.touches {
            let _ = sessions.confirm_touch_released(&self.lease, touch, now);
        }
    }
    pub fn cleanup(&mut self, sessions: &mut SessionBroker, now: u64) {
        self.composition.cancel();
        let _ = sessions.set_composition(&self.lease, None, now);
        self.cleanup_router(sessions, now);
    }
    pub fn validate_raw(&self, command: &RawInputCommand, route: &Route) -> Result<(), Rejection> {
        if command.seat_id != FIXTURE_SEAT || command.device_id != FIXTURE_DEVICE {
            return Err(Rejection::Unauthorized);
        }
        if self
            .trace
            .len()
            .checked_add(command.batch.events.len())
            .is_none_or(|n| n > MAX_RAW_SAMPLES)
        {
            return Err(Rejection::InvalidArguments);
        }
        for event in &command.batch.events {
            // This source negotiates CLIENT_TEXT and implements one pointer
            // button. Native host-layout shortcuts belong to another producer.
            match event.event {
                Event::Key { .. } => return Err(Rejection::UnsupportedAction),
                Event::Button { code, .. } if code != 1 => return Err(Rejection::InvalidArguments),
                _ => {}
            }
        }
        self.router
            .validate(route, &command.batch)
            .map_err(rejection)
    }
    pub fn dispatch_raw(
        &mut self,
        command: &RawInputCommand,
        route: &Route,
        sessions: &mut SessionBroker,
        now: u64,
        mut apply: impl FnMut(&TimedEvent),
    ) -> Result<(), InputError> {
        self.validate_raw(command, route)
            .map_err(|_| InputError::Unauthorized)?;
        sessions
            .validate_family_control(
                &self.lease.client,
                &command.header,
                CommandFamily::RawInput,
                "RawInputBatch",
                now,
            )
            .map_err(|_| InputError::Unauthorized)?;
        // Reserve every possible press in the common cleanup owner BEFORE any
        // source callback, including press/release pairs in this same batch.
        for timed in &command.batch.events {
            match timed.event {
                Event::Button {
                    code,
                    pressed: true,
                    ..
                } => sessions.record_button(&self.lease, code, true, now),
                Event::TouchDown { id, .. } => sessions.record_touch(&self.lease, id, true, now),
                _ => Ok(()),
            }
            .map_err(|_| InputError::Limit)?;
        }
        let trace = &mut self.trace;
        let sequence = command.header.command_seq;
        let mut index = 0;
        self.router.dispatch(route, &command.batch, |event| {
            trace.push(RawSample {
                command_sequence: sequence,
                event_index: index,
                event: event.clone(),
            });
            index += 1;
            apply(event);
            Ok(())
        })?;
        // Releases are confirmed only after every source callback succeeded.
        for timed in &command.batch.events {
            match timed.event {
                Event::Button {
                    code,
                    pressed: false,
                    ..
                } if !self.router.held().buttons.contains(&code) => {
                    sessions.confirm_button_released(&self.lease, code, now)
                }
                Event::TouchUp { id } | Event::TouchCancel { id }
                    if !self.router.held().touches.contains(&id) =>
                {
                    sessions.confirm_touch_released(&self.lease, id, now)
                }
                _ => Ok(()),
            }
            .map_err(|_| InputError::OutcomeUnknown)?;
        }
        Ok(())
    }
    pub fn validate_composition(&self, command: &TextCompositionCommand) -> Result<(), Rejection> {
        match &command.operation {
            CompositionOperation::Begin(range) => self.composition.validate_begin(
                command.text_session_id,
                command.focus_generation,
                command.base_text_version,
                *range,
            ),
            CompositionOperation::UpdatePreedit { text, selection } => self
                .composition
                .validate_update(command.text_session_id, text, *selection),
            CompositionOperation::CommitText { text } => self.composition.validate_commit(
                command.text_session_id,
                command.focus_generation,
                command.base_text_version,
                text,
            ),
            CompositionOperation::Cancel => {
                if self.composition.active_id() == Some(command.text_session_id) {
                    Ok(())
                } else {
                    Err(InputError::StaleComposition)
                }
            }
        }
        .map_err(rejection)
    }
    /// Returns an edit only for Commit; other operations are transient.
    pub fn compose(
        &mut self,
        command: &TextCompositionCommand,
        sessions: &mut SessionBroker,
        now: u64,
    ) -> Result<Option<TextEdit>, InputError> {
        self.validate_composition(command)
            .map_err(|_| InputError::StaleComposition)?;
        let edit = match &command.operation {
            CompositionOperation::Begin(range) => {
                self.composition.begin(
                    command.text_session_id,
                    command.focus_generation,
                    command.base_text_version,
                    *range,
                )?;
                sessions
                    .set_composition(&self.lease, Some(command.text_session_id), now)
                    .map_err(|_| InputError::OutcomeUnknown)?;
                None
            }
            CompositionOperation::UpdatePreedit { text, selection } => {
                self.composition
                    .update(command.text_session_id, text, *selection)?;
                None
            }
            CompositionOperation::CommitText { text } => Some(self.composition.commit(
                command.text_session_id,
                command.focus_generation,
                command.base_text_version,
                text,
            )?),
            CompositionOperation::Cancel => {
                self.composition.cancel();
                None
            }
        };
        if self.composition.active_id().is_none() {
            sessions
                .set_composition(&self.lease, None, now)
                .map_err(|_| InputError::OutcomeUnknown)?;
        }
        Ok(edit)
    }
}
fn rejection(error: InputError) -> Rejection {
    match error {
        InputError::StaleRoute => Rejection::StaleTarget,
        InputError::StaleComposition => Rejection::StaleText,
        InputError::Unauthorized | InputError::TextOwnership => Rejection::Unauthorized,
        _ => Rejection::InvalidArguments,
    }
}
