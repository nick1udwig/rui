//! The shared admission boundary and receiver for the shell's private native
//! device. Transport hosts inject their existing broker and command ledger.
use crate::{
    native::{Endpoint, NativeEvent},
    topology::Topology,
    RegionGrant, ShellError,
};
use rui_protocol_core::{
    commands::{
        AuthenticatedClient, Command, CommandFamily, CommandLedger, CommandResult, DispatchFailure,
        DispatchOutcome, Id, RawInputCommand, Rejection,
    },
    input::{Event, HeldInput, InputLimits, InputRouter},
    model::Point,
    resources::{MemoryBudget, MemoryReservation},
    session::{ControlLease, PhysicalKey, SessionBroker},
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

pub const SEAT: &str = "rui-isolated-seat";
pub const DEVICE: &str = "rui-private-device";
const MOTION: u32 = 400;
const BUTTON: u32 = 500;
const SCROLL: u32 = 600;
const KEY: u32 = 700;
const DOWN: u32 = 800;
const MOVE: u32 = 802;
const UP: u32 = 801;
const FRAME: u32 = 100;
const MAX_FRAMES: usize = 512;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receipt {
    AppliedToSource,
    Cancelled,
}
/// Metadata only; source text and authorization credentials are never traced.
#[derive(Clone, Copy, Debug)]
pub struct NativeTrace {
    pub command_sequence: u64,
    pub event_index: u16,
    pub emulation_sequence: u32,
    pub native_time_us: u64,
    pub surface: u64,
    pub receipt: Receipt,
}
#[derive(Clone, Debug)]
pub struct NativeProfile {
    pub libei: &'static str,
    pub libeis: &'static str,
    pub xkbcommon: &'static str,
    pub keymap_sha256: [u8; 32],
    pub host_layout: String,
    pub repeat_delay_ms: u64,
    pub repeat_interval_ms: u64,
    pub region: [u32; 4],
}
struct Expected {
    surface: u64,
    token: Id,
    lease: Id,
    generation: u64,
    sequence: u32,
    command: u64,
    index: u16,
    events: [NativeEvent; 2],
    count: usize,
    touch: u64,
    cleanup: bool,
    hit_required: bool,
    work_generation: u64,
}
struct Repeat {
    surface: u64,
    token: Id,
    lease: Id,
    generation: u64,
    next: u64,
    work_generation: u64,
}
/// One owning thread, one explicit whole-session region and one native device.
/// All mutating native input enters through `submit_raw`; topology is trusted
/// producer state, not an authorization channel exposed to a wire peer.
pub struct Backend {
    endpoint: Endpoint,
    topology: Topology,
    router: Option<InputRouter>,
    router_surface: Option<u64>,
    router_token: Id,
    active: Option<(Id, u64)>,
    sequence: u32,
    expected: VecDeque<Expected>,
    received: [NativeEvent; 2],
    received_count: usize,
    touches: BTreeMap<u64, u64>,
    source_held: HeldInput,
    repeats: BTreeMap<u32, Repeat>,
    traces: VecDeque<NativeTrace>,
    profile: NativeProfile,
    failed: bool,
    cleanup_deadline: Option<Instant>,
    work_generation: Arc<AtomicU64>,
    _memory: MemoryReservation,
}
impl Backend {
    pub fn new(
        grant: RegionGrant,
        layout: &str,
        budget: &MemoryBudget,
    ) -> Result<Self, ShellError> {
        let memory = budget.reserve(1024 * 1024)?;
        let mut endpoint = Endpoint::new(grant, layout, budget)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while !endpoint.ready() {
            endpoint.next()?;
            if Instant::now() >= deadline {
                return Err(ShellError::Native("handshake timeout"));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let region = endpoint.region()?;
        let r = grant.granted();
        if region != [r.x as u32, r.y as u32, r.width as u32, r.height as u32] {
            return Err(ShellError::Geometry);
        }
        let profile = NativeProfile {
            libei: "1.6.0",
            libeis: "1.6.0",
            xkbcommon: "1.13.2",
            keymap_sha256: endpoint.keymap_digest()?,
            host_layout: layout.into(),
            repeat_delay_ms: 400,
            repeat_interval_ms: 40,
            region,
        };
        Ok(Self {
            endpoint,
            topology: Topology::new(grant, budget)?,
            router: None,
            router_surface: None,
            router_token: [0; 16],
            active: None,
            sequence: 0,
            expected: VecDeque::new(),
            received: [NativeEvent::default(); 2],
            received_count: 0,
            touches: BTreeMap::new(),
            source_held: HeldInput::default(),
            repeats: BTreeMap::new(),
            traces: VecDeque::new(),
            profile,
            failed: false,
            cleanup_deadline: None,
            work_generation: Arc::new(AtomicU64::new(1)),
            _memory: memory,
        })
    }
    /// Trusted owner binds priority invalidation before any native emulation.
    pub fn bind_generation(&mut self, generation: Arc<AtomicU64>) -> Result<(), ShellError> {
        if self.active.is_some() || !self.expected.is_empty() {
            return Err(ShellError::Scope);
        }
        self.work_generation = generation;
        Ok(())
    }
    pub fn topology(&self) -> &Topology {
        &self.topology
    }
    /// The caller is the trusted source producer. Route changes are checked
    /// again at receiving frames, including while native work is in transit.
    pub fn topology_mut(&mut self) -> &mut Topology {
        &mut self.topology
    }
    pub fn profile(&self) -> &NativeProfile {
        &self.profile
    }
    pub fn traces(&self) -> impl Iterator<Item = &NativeTrace> {
        self.traces.iter()
    }
    pub fn held(&self) -> &HeldInput {
        &self.source_held
    }
    pub fn pending_frames(&self) -> usize {
        self.expected.len()
    }
    pub fn validate_source(&mut self, command: &RawInputCommand) -> Result<(), ShellError> {
        if self.failed || self.cleanup_deadline.is_some() {
            return Err(ShellError::Native("backend requires cleanup or disposal"));
        }
        if command.seat_id != SEAT || command.device_id != DEVICE {
            return Err(ShellError::Scope);
        }
        if self.expected.len() + command.batch.events.len() > MAX_FRAMES {
            return Err(ShellError::Limit);
        }
        let route = self
            .topology
            .route(command.batch.surface, command.batch.presented_revision)?;
        // A temporary router validates the entire batch without sending. A
        // matching live router retains physical transitions across commands.
        if self.router.as_ref().is_some_and(|r| {
            !r.held().keys.is_empty()
                || !r.held().buttons.is_empty()
                || !r.held().touches.is_empty()
        }) && self.active
            != Some((
                command.header.control_lease_id,
                command.header.control_generation,
            ))
        {
            return Err(ShellError::Scope);
        }
        let mut held = self
            .router
            .as_ref()
            .map(|r| r.held().clone())
            .unwrap_or_default();
        for event in &command.batch.events {
            if fresh_hit(&event.event, &held)
                && !self
                    .topology
                    .point_hits(command.batch.surface, event_point(&event.event).unwrap())?
            {
                return Err(ShellError::Input(
                    rui_protocol_core::input::InputError::Unauthorized,
                ));
            }
            update_held(&mut held, &event.event);
            self.convert(command.batch.surface, &event.event)?;
            if let Event::Key { code, .. } = event.event {
                if !self.endpoint.has_key(code) {
                    return Err(ShellError::Native("unsupported physical key"));
                }
            }
        }
        // Reuse the core's COMPLETE batch preflight before START as well as
        // before each dispatch: no timestamp/held error can reach native code.
        if self.router_surface == Some(command.batch.surface) {
            self.router
                .as_ref()
                .unwrap()
                .validate(&route, &command.batch)?;
        } else {
            if self
                .router
                .as_ref()
                .is_some_and(|r| held_count(r.held()) != 0)
            {
                return Err(ShellError::Scope);
            }
            InputRouter::new(command.batch.surface, InputLimits::default())?
                .validate(&route, &command.batch)?;
        }
        Ok(())
    }
    /// Uses the SAME injected session/ledger as semantic, resize and IME paths.
    /// Exact retries return the ledger result and do not resend native frames.
    pub fn submit_raw(
        &mut self,
        sessions: &mut SessionBroker,
        ledger: &mut CommandLedger,
        client: &AuthenticatedClient,
        bytes: &[u8],
        now_ms: u64,
    ) -> Result<CommandResult, ShellError> {
        let generation = self.work_generation.load(Ordering::Acquire);
        self.submit_raw_at_generation(sessions, ledger, client, bytes, now_ms, generation)
    }
    /// Owner-queued work keeps its ORIGINAL invalidation generation. Never
    /// relabel a stale queued command with a newer local editing generation.
    pub fn submit_raw_at_generation(
        &mut self,
        sessions: &mut SessionBroker,
        ledger: &mut CommandLedger,
        client: &AuthenticatedClient,
        bytes: &[u8],
        now_ms: u64,
        generation: u64,
    ) -> Result<CommandResult, ShellError> {
        if generation != self.work_generation.load(Ordering::Acquire) {
            return Err(ShellError::Native("stale owner generation"));
        }
        self.synchronize(sessions, now_ms)?;
        let this = RefCell::new(self);
        Ok(ledger.submit_controlled_with_session(
            sessions,
            client,
            CommandFamily::RawInput,
            bytes,
            now_ms,
            |command| {
                let Command::RawInput(c) = command else {
                    return Err(Rejection::UnsupportedAction);
                };
                this.borrow_mut()
                    .validate_source(c)
                    .map_err(|_| Rejection::InvalidArguments)
            },
            |command, sessions| {
                let Command::RawInput(c) = command else {
                    return Err(DispatchFailure::Failed);
                };
                this.borrow_mut()
                    .dispatch_at_generation(c, sessions, now_ms, generation)
            },
        )?)
    }
    /// Trusted host callback after the ONE common ledger admission. This path
    /// repeats current source/native checks but never admits the sequence again.
    pub fn dispatch_admitted(
        &mut self,
        command: &RawInputCommand,
        sessions: &mut SessionBroker,
        now: u64,
    ) -> Result<DispatchOutcome, DispatchFailure> {
        let generation = self.work_generation.load(Ordering::Acquire);
        self.dispatch_at_generation(command, sessions, now, generation)
    }
    pub fn dispatch_at_generation(
        &mut self,
        command: &RawInputCommand,
        sessions: &mut SessionBroker,
        now: u64,
        generation: u64,
    ) -> Result<DispatchOutcome, DispatchFailure> {
        if generation != self.work_generation.load(Ordering::Acquire) {
            return Err(DispatchFailure::Failed);
        }
        let result = self.dispatch_inner(command, sessions, now, generation);
        if result == Err(DispatchFailure::OutcomeUnknown) {
            self.failed = true;
        }
        result
    }
    fn dispatch_inner(
        &mut self,
        command: &RawInputCommand,
        sessions: &mut SessionBroker,
        now: u64,
        generation: u64,
    ) -> Result<DispatchOutcome, DispatchFailure> {
        self.synchronize(sessions, now)
            .map_err(|_| DispatchFailure::OutcomeUnknown)?;
        self.validate_source(command)
            .map_err(|_| DispatchFailure::Failed)?;
        if command.batch.presented_revision != command.header.observed_view_revision {
            return Err(DispatchFailure::Failed);
        }
        let lease = sessions.current_lease(now).ok_or(DispatchFailure::Failed)?;
        if lease.lease_id != command.header.control_lease_id
            || lease.generation != command.header.control_generation
            || lease.view_id != command.header.view_id
            || lease.client.client_instance_id != command.header.client_instance_id
        {
            return Err(DispatchFailure::Failed);
        }
        if sessions
            .presented_revision(&lease.client, lease.view_id)
            .map_err(|_| DispatchFailure::Failed)?
            != Some(command.batch.presented_revision)
        {
            return Err(DispatchFailure::Failed);
        }
        sessions
            .authorize_presented(
                &lease.client,
                lease.view_id,
                command.batch.presented_revision,
            )
            .map_err(|_| DispatchFailure::Failed)?;
        let route = self
            .topology
            .route(command.batch.surface, command.batch.presented_revision)
            .map_err(|_| DispatchFailure::Failed)?;
        let binding = (lease.lease_id, lease.generation);
        if self.active != Some(binding) {
            if self.active.is_some() {
                return Err(DispatchFailure::Failed);
            }
            self.sequence = self
                .sequence
                .checked_add(1)
                .ok_or(DispatchFailure::Failed)?;
            self.endpoint
                .start(self.sequence)
                .map_err(|_| DispatchFailure::OutcomeUnknown)?;
            self.active = Some(binding);
        }
        // Changing captured surfaces is allowed only after neutralization.
        if self
            .router
            .as_ref()
            .is_none_or(|_| self.router_surface != Some(command.batch.surface))
        {
            if self
                .router
                .as_ref()
                .is_some_and(|r| held_count(r.held()) != 0)
            {
                return Err(DispatchFailure::Failed);
            }
            self.router = Some(
                InputRouter::new(command.batch.surface, InputLimits::default())
                    .map_err(|_| DispatchFailure::Failed)?,
            );
            self.router_surface = Some(command.batch.surface);
        }
        self.router_token = route.token;
        let mut router = self.router.take().unwrap();
        let mut held = router.held().clone();
        let mut event_index = 0u16;
        let result = router.dispatch(&route, &command.batch, |timed| {
            let index = event_index;
            event_index += 1;
            let hit_required = fresh_hit(&timed.event, &held);
            update_held(&mut held, &timed.event);
            let (events, count, touch) = self
                .convert(command.batch.surface, &timed.event)
                .map_err(|_| ())?;
            // Retain possible held state in the common broker before an
            // external call, never release its ownership on ambiguous send.
            record_press(sessions, &lease, &timed.event, now).map_err(|_| ())?;
            let cleanup = matches!(
                timed.event,
                Event::Key { pressed: false, .. }
                    | Event::Button { pressed: false, .. }
                    | Event::TouchUp { .. }
                    | Event::TouchCancel { .. }
            );
            self.expected.push_back(Expected {
                surface: command.batch.surface,
                token: route.token,
                lease: lease.lease_id,
                generation: lease.generation,
                sequence: self.sequence,
                command: command.header.command_seq,
                index,
                events,
                count,
                touch,
                cleanup,
                hit_required,
                work_generation: generation,
            });
            for event in &events[..count] {
                self.endpoint.send(*event).map_err(|_| ())?;
            }
            self.endpoint.frame();
            Ok(())
        });
        self.router = Some(router);
        result.map_err(|e| {
            if e == rui_protocol_core::input::InputError::OutcomeUnknown {
                DispatchFailure::OutcomeUnknown
            } else {
                DispatchFailure::Failed
            }
        })?;
        Ok(DispatchOutcome::Dispatched)
    }
    fn convert(
        &self,
        surface: u64,
        event: &Event,
    ) -> Result<([NativeEvent; 2], usize, u64), ShellError> {
        let mut out = [NativeEvent::default(); 2];
        let mut count = 1;
        let mut touch = 0;
        let point = |p| self.topology.native_point(surface, p);
        let motion = |p: Point| NativeEvent {
            kind: MOTION,
            x: p.x,
            y: p.y,
            ..Default::default()
        };
        match *event {
            Event::Motion(p) => out[0] = motion(point(p)?),
            Event::Button {
                code,
                pressed,
                position,
            } => {
                if !(1..=8).contains(&code) {
                    return Err(ShellError::Native("unsupported button"));
                }
                let button = NativeEvent {
                    kind: BUTTON,
                    code: 0x10f + code,
                    pressed: u32::from(pressed),
                    ..Default::default()
                };
                if pressed {
                    out[0] = motion(point(position)?);
                    out[1] = button;
                    count = 2;
                } else {
                    out[0] = button;
                }
            }
            Event::Wheel { x, y, position } => {
                out[0] = motion(point(position)?);
                out[1] = NativeEvent {
                    kind: SCROLL,
                    x,
                    y,
                    ..Default::default()
                };
                count = 2;
            }
            Event::Key { code, pressed } => {
                out[0] = NativeEvent {
                    kind: KEY,
                    code,
                    pressed: u32::from(pressed),
                    ..Default::default()
                }
            }
            Event::TouchDown { id, position } | Event::TouchMove { id, position } => {
                let p = point(position)?;
                touch = id;
                out[0] = NativeEvent {
                    kind: if matches!(event, Event::TouchDown { .. }) {
                        DOWN
                    } else {
                        MOVE
                    },
                    touch_id: id,
                    x: p.x,
                    y: p.y,
                    ..Default::default()
                };
            }
            Event::TouchUp { id } | Event::TouchCancel { id } => {
                touch = id;
                out[0] = NativeEvent {
                    kind: UP,
                    touch_id: id,
                    pressed: u32::from(matches!(event, Event::TouchCancel { .. })),
                    ..Default::default()
                };
            }
        }
        Ok((out, count, touch))
    }
    fn current(&self, expected: &Expected, sessions: &mut SessionBroker, now: u64) -> bool {
        expected.work_generation == self.work_generation.load(Ordering::Acquire)
            && self.active == Some((expected.lease, expected.generation))
            && sessions.current_lease(now).is_some_and(|l| {
                l.lease_id == expected.lease
                    && l.generation == expected.generation
                    && sessions
                        .presented_revision(&l.client, l.view_id)
                        .is_ok_and(|p| p.is_some())
            })
            && (!expected.hit_required
                || self
                    .topology
                    .grant()
                    .unmap(Point {
                        x: expected.events[0].x,
                        y: expected.events[0].y,
                    })
                    .is_ok_and(|p| self.topology.hit_test(p) == Some(expected.surface)))
            && self.topology.route(expected.surface, 1).is_ok_and(|r| {
                r.token == expected.token
                    && r.mapped
                    && !r.blocked_by_modal
                    && (r.focused
                        || expected.events[..expected.count]
                            .iter()
                            .all(|e| e.kind != KEY))
            })
    }
    /// Nonblocking bounded pump; actual EIS FRAME records establish delivery.
    /// It checks current authority before applying native effects to the source.
    pub fn poll(&mut self, sessions: &mut SessionBroker, now_ms: u64) -> Result<usize, ShellError> {
        let result = self.poll_inner(sessions, now_ms);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn poll_inner(
        &mut self,
        sessions: &mut SessionBroker,
        now_ms: u64,
    ) -> Result<usize, ShellError> {
        self.synchronize(sessions, now_ms)?;
        self.pump_frames(sessions, now_ms)
    }
    fn pump_frames(
        &mut self,
        sessions: &mut SessionBroker,
        now_ms: u64,
    ) -> Result<usize, ShellError> {
        let mut frames = 0;
        for _ in 0..2048 {
            let Some(event) = self.endpoint.next()? else {
                break;
            };
            if [MOTION, BUTTON, SCROLL, KEY, DOWN, MOVE, UP].contains(&event.kind) {
                if self.received_count >= 2 {
                    return Err(ShellError::Native("unexpected native batch"));
                }
                self.received[self.received_count] = event;
                self.received_count += 1;
            } else if event.kind == FRAME {
                self.receive_frame(event, sessions, now_ms, false)?;
                frames += 1;
            }
        }
        self.repeat(sessions, now_ms)?;
        Ok(frames)
    }
    fn receive_frame(
        &mut self,
        event: NativeEvent,
        sessions: &mut SessionBroker,
        now: u64,
        cancel_all: bool,
    ) -> Result<(), ShellError> {
        let expected = self
            .expected
            .pop_front()
            .ok_or(ShellError::Native("unmatched native frame"))?;
        if expected.sequence != event.sequence || expected.count != self.received_count {
            return Err(ShellError::Native("native frame sequence or size"));
        }
        for (actual, wanted) in self.received[..self.received_count]
            .iter()
            .zip(&expected.events[..expected.count])
        {
            if actual.kind != wanted.kind
                || actual.code != wanted.code
                || actual.pressed != wanted.pressed
                || (actual.x - wanted.x).abs() > 1e-4
                || (actual.y - wanted.y).abs() > 1e-4
            {
                return Err(ShellError::Native("native event mismatch"));
            }
            if [MOVE, UP].contains(&actual.kind)
                && self.touches.get(&expected.touch) != Some(&actual.touch_id)
            {
                return Err(ShellError::Native("native touch identity"));
            }
            if actual.kind == DOWN {
                self.touches.insert(expected.touch, actual.touch_id);
            }
        }
        let allowed = !cancel_all && self.current(&expected, sessions, now);
        let received = self.received;
        self.received_count = 0;
        if allowed || expected.cleanup {
            for actual in &received[..expected.count] {
                self.apply_native(&expected, *actual, allowed, now)?;
            }
        }
        // A stale native down still occupies native touch state until its
        // matched up/cancel. Track that identity separately from source effects.
        if received[..expected.count].iter().any(|e| e.kind == UP) {
            self.touches.remove(&expected.touch);
        }
        if expected.cleanup {
            if let Some(lease) = sessions
                .current_lease(now)
                .filter(|l| l.lease_id == expected.lease && l.generation == expected.generation)
            {
                for actual in &received[..expected.count] {
                    match actual.kind {
                        KEY => sessions
                            .confirm_key_released(
                                &lease,
                                &PhysicalKey {
                                    namespace: "linux.evdev".into(),
                                    code: actual.code,
                                },
                                now,
                            )
                            .map_err(|_| ShellError::Native("broker key release"))?,
                        BUTTON => sessions
                            .confirm_button_released(&lease, actual.code - 0x10f, now)
                            .map_err(|_| ShellError::Native("broker button release"))?,
                        UP => sessions
                            .confirm_touch_released(&lease, expected.touch, now)
                            .map_err(|_| ShellError::Native("broker touch release"))?,
                        _ => {}
                    }
                }
            }
        }
        if self.traces.len() == 256 {
            self.traces.pop_front();
        }
        self.traces.push_back(NativeTrace {
            command_sequence: expected.command,
            event_index: expected.index,
            emulation_sequence: event.sequence,
            native_time_us: event.time,
            surface: expected.surface,
            receipt: if allowed {
                Receipt::AppliedToSource
            } else {
                Receipt::Cancelled
            },
        });
        Ok(())
    }
    fn apply_native(
        &mut self,
        job: &Expected,
        event: NativeEvent,
        allowed: bool,
        now: u64,
    ) -> Result<(), ShellError> {
        match event.kind {
            MOTION if allowed => self.topology.source_pointer(Point {
                x: event.x,
                y: event.y,
            })?,
            KEY => {
                let pressed = event.pressed != 0;
                let text = self.endpoint.key(event.code, pressed, true)?;
                if pressed {
                    self.source_held.keys.insert(event.code);
                    if allowed && self.endpoint.repeats(event.code) {
                        self.repeats.insert(
                            event.code,
                            Repeat {
                                surface: job.surface,
                                token: job.token,
                                lease: job.lease,
                                generation: job.generation,
                                next: now.saturating_add(self.profile.repeat_delay_ms),
                                work_generation: job.work_generation,
                            },
                        );
                    }
                } else {
                    self.source_held.keys.remove(&event.code);
                    self.repeats.remove(&event.code);
                }
                if allowed {
                    self.topology.source_activity(job.surface, &text)?;
                }
            }
            BUTTON => {
                let code = event.code - 0x10f;
                if event.pressed != 0 {
                    self.source_held.buttons.insert(code);
                } else {
                    self.source_held.buttons.remove(&code);
                }
                if allowed {
                    self.topology.source_activity(job.surface, "")?;
                }
            }
            DOWN => {
                self.touches.insert(job.touch, event.touch_id);
                self.source_held.touches.insert(job.touch);
                if allowed {
                    self.topology.source_activity(job.surface, "")?;
                }
            }
            MOVE | UP => {
                if event.kind == UP {
                    self.touches.remove(&job.touch);
                    self.source_held.touches.remove(&job.touch);
                }
                if allowed {
                    self.topology.source_activity(job.surface, "")?;
                }
            }
            SCROLL if allowed => self.topology.source_activity(job.surface, "")?,
            _ => {}
        }
        Ok(())
    }
    fn repeat(&mut self, sessions: &mut SessionBroker, now: u64) -> Result<(), ShellError> {
        let lease = sessions.current_lease(now);
        let mut burst = 0;
        for (&code, repeat) in &mut self.repeats {
            let valid = repeat.work_generation == self.work_generation.load(Ordering::Acquire)
                && lease.as_ref().is_some_and(|l| {
                    l.lease_id == repeat.lease
                        && l.generation == repeat.generation
                        && native_ready(sessions, l)
                })
                && self.topology.route(repeat.surface, 1).is_ok_and(|r| {
                    r.token == repeat.token && r.mapped && r.focused && !r.blocked_by_modal
                });
            if !valid {
                continue;
            }
            while now >= repeat.next && burst < 32 {
                let text = self.endpoint.key(code, true, false)?;
                self.topology.source_activity(repeat.surface, &text)?;
                repeat.next = repeat.next.saturating_add(self.profile.repeat_interval_ms);
                burst += 1;
            }
        }
        Ok(())
    }
    fn synchronize(&mut self, sessions: &mut SessionBroker, now: u64) -> Result<(), ShellError> {
        if self.failed {
            return Err(ShellError::Native(
                "backend requires disposal after failed cleanup",
            ));
        }
        let lease = sessions.current_lease(now);
        let current = lease.as_ref().map(|l| (l.lease_id, l.generation));
        let ready = lease.as_ref().is_some_and(|l| native_ready(sessions, l));
        let stale_capture = self
            .router
            .as_ref()
            .is_some_and(|r| held_count(r.held()) != 0)
            && self.router_surface.is_some_and(|surface| {
                self.topology
                    .route(surface, 1)
                    .map_or(true, |r| r.token != self.router_token)
            });
        if self.active.is_some() && (self.active != current || stale_capture || !ready) {
            self.neutralize(sessions, now)?;
        }
        // Central host fanout owns common cleanup, including other families.
        // This raw backend must never consume or acknowledge that record.
        Ok(())
    }
    /// Source-local trusted input handler, not a client-invocable command.
    /// Revocation happens before draining native work; a new grant remains
    /// blocked by the broker until actual held-state neutralization succeeds.
    pub fn local_input(
        &mut self,
        sessions: &mut SessionBroker,
        now: u64,
    ) -> Result<(), ShellError> {
        sessions.local_activity(now);
        self.neutralize(sessions, now)?;
        Ok(())
    }
    fn begin_neutralization(&mut self) -> Result<(), ShellError> {
        if self.cleanup_deadline.is_some() {
            return Ok(());
        }
        self.repeats.clear();
        let Some((lease, generation)) = self.active else {
            if self.failed {
                return Err(ShellError::Native(
                    "dispose backend after untracked native failure",
                ));
            }
            return Ok(());
        };
        self.failed = true;
        if let Some(router) = &mut self.router {
            let held = router.drain_cleanup();
            let surface = self.router_surface.unwrap();
            let release = |event: NativeEvent, touch: u64| Expected {
                surface,
                token: [0; 16],
                lease,
                generation,
                sequence: self.sequence,
                command: 0,
                index: 0,
                events: [event, NativeEvent::default()],
                count: 1,
                touch,
                cleanup: true,
                hit_required: false,
                work_generation: self.work_generation.load(Ordering::Acquire),
            };
            for code in held.keys {
                let e = NativeEvent {
                    kind: KEY,
                    code,
                    ..Default::default()
                };
                self.expected.push_back(release(e, 0));
                self.endpoint.send(e)?;
                self.endpoint.frame();
            }
            for code in held.buttons {
                let e = NativeEvent {
                    kind: BUTTON,
                    code: 0x10f + code,
                    ..Default::default()
                };
                self.expected.push_back(release(e, 0));
                self.endpoint.send(e)?;
                self.endpoint.frame();
            }
            for id in held.touches {
                let e = NativeEvent {
                    kind: UP,
                    touch_id: id,
                    pressed: 1,
                    ..Default::default()
                };
                self.expected.push_back(release(e, id));
                self.endpoint.send(e)?;
                self.endpoint.frame();
            }
        }
        self.endpoint.stop();
        self.cleanup_deadline = Some(Instant::now() + Duration::from_secs(2));
        Ok(())
    }
    fn advance_cleanup(
        &mut self,
        sessions: &mut SessionBroker,
        now: u64,
    ) -> Result<bool, ShellError> {
        let Some(deadline) = self.cleanup_deadline else {
            return Ok(true);
        };
        let mut stopped = false;
        for _ in 0..2048 {
            match self.endpoint.next()? {
                Some(event) if event.kind == 201 => {
                    if event.sequence != self.sequence
                        || !self.expected.is_empty()
                        || self.received_count != 0
                    {
                        return Err(ShellError::Native("incomplete native cleanup"));
                    }
                    stopped = true;
                    break;
                }
                Some(event)
                    if [MOTION, BUTTON, SCROLL, KEY, DOWN, MOVE, UP].contains(&event.kind) =>
                {
                    if self.received_count >= 2 {
                        return Err(ShellError::Native("unexpected cleanup batch"));
                    }
                    self.received[self.received_count] = event;
                    self.received_count += 1;
                }
                Some(event) if event.kind == FRAME => {
                    self.receive_frame(event, sessions, now, true)?
                }
                Some(_) => {}
                None => {
                    if Instant::now() >= deadline {
                        return Err(ShellError::Native("neutralization timeout"));
                    }
                    return Ok(false);
                }
            }
        }
        if !stopped {
            if Instant::now() >= deadline {
                return Err(ShellError::Native("neutralization timeout"));
            }
            return Ok(false);
        }
        if !self.source_held.keys.is_empty()
            || !self.source_held.buttons.is_empty()
            || !self.source_held.touches.is_empty()
            || !self.touches.is_empty()
        {
            return Err(ShellError::Native("held input after cleanup"));
        }
        self.endpoint.clear_keys()?;
        self.router = None;
        self.router_surface = None;
        self.active = None;
        self.failed = false;
        self.cleanup_deadline = None;
        Ok(true)
    }
    fn neutralize(&mut self, sessions: &mut SessionBroker, now: u64) -> Result<(), ShellError> {
        self.begin_neutralization()?;
        while !self.advance_cleanup(sessions, now)? {
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
    pub fn cleanup_pending(&self) -> bool {
        self.cleanup_deadline.is_some()
    }
    pub(crate) fn source_local_key(
        &mut self,
        code: u32,
        pressed: bool,
        update: bool,
    ) -> Result<(), ShellError> {
        if self.failed || self.cleanup_pending() || !self.endpoint.has_key(code) {
            return Err(ShellError::Native("local device is unready"));
        }
        let surface = self.topology.focused_surface().ok_or(ShellError::Scope)?;
        let text = self.endpoint.key(code, pressed, update)?;
        self.topology.source_activity(surface, &text)
    }
    pub(crate) fn clear_local_keys(&mut self) -> Result<(), ShellError> {
        if self.failed || self.cleanup_pending() {
            return Err(ShellError::Native("native cleanup pending"));
        }
        self.endpoint.clear_keys()
    }
    /// Begin the same verified cleanup without waiting on native I/O. The
    /// owner loop continues serving priority invalidation while it advances.
    pub fn invalidate_local_input(
        &mut self,
        sessions: &mut SessionBroker,
        now: u64,
    ) -> Result<(), ShellError> {
        sessions.local_activity(now);
        self.begin_neutralization()
    }
    /// Bounded nonblocking owner-loop pump. Fresh commands must wait while
    /// cleanup_pending is true; central common cleanup remains unacknowledged.
    pub fn poll_nonblocking(
        &mut self,
        sessions: &mut SessionBroker,
        now: u64,
    ) -> Result<usize, ShellError> {
        let result = self.poll_nonblocking_inner(sessions, now);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn poll_nonblocking_inner(
        &mut self,
        sessions: &mut SessionBroker,
        now: u64,
    ) -> Result<usize, ShellError> {
        if self.cleanup_pending() {
            self.advance_cleanup(sessions, now)?;
            return Ok(0);
        }
        if self.failed {
            return Err(ShellError::Native("backend requires cleanup or disposal"));
        }
        let lease = sessions.current_lease(now);
        let current = lease.as_ref().map(|l| (l.lease_id, l.generation));
        let ready = lease.as_ref().is_some_and(|l| native_ready(sessions, l));
        let stale = self
            .router
            .as_ref()
            .is_some_and(|r| held_count(r.held()) != 0)
            && self.router_surface.is_some_and(|surface| {
                self.topology
                    .route(surface, 1)
                    .map_or(true, |r| r.token != self.router_token)
            });
        if self.active.is_some() && (self.active != current || !ready || stale) {
            self.begin_neutralization()?;
            self.advance_cleanup(sessions, now)?;
            return Ok(0);
        }
        self.pump_frames(sessions, now)
    }
}
fn held_count(held: &HeldInput) -> usize {
    held.keys.len() + held.buttons.len() + held.touches.len()
}
fn native_ready(sessions: &SessionBroker, lease: &ControlLease) -> bool {
    sessions
        .presented_revision(&lease.client, lease.view_id)
        .is_ok_and(|revision| {
            revision.is_some_and(|revision| {
                sessions
                    .authorize_presented(&lease.client, lease.view_id, revision)
                    .is_ok()
            })
        })
}
fn record_press(
    sessions: &mut SessionBroker,
    lease: &ControlLease,
    event: &Event,
    now: u64,
) -> Result<(), ()> {
    match *event {
        Event::Key {
            code,
            pressed: true,
        } => sessions
            .record_key(
                lease,
                PhysicalKey {
                    namespace: "linux.evdev".into(),
                    code,
                },
                true,
                now,
            )
            .map_err(|_| ()),
        Event::Button {
            code,
            pressed: true,
            ..
        } => sessions
            .record_button(lease, code, true, now)
            .map_err(|_| ()),
        Event::TouchDown { id, .. } => sessions.record_touch(lease, id, true, now).map_err(|_| ()),
        _ => Ok(()),
    }
}

fn event_point(event: &Event) -> Option<Point> {
    match *event {
        Event::Motion(p)
        | Event::Button { position: p, .. }
        | Event::Wheel { position: p, .. }
        | Event::TouchDown { position: p, .. }
        | Event::TouchMove { position: p, .. } => Some(p),
        _ => None,
    }
}
fn fresh_hit(event: &Event, held: &HeldInput) -> bool {
    match *event {
        Event::Motion(_) => held.buttons.is_empty(),
        Event::Button { pressed, .. } => pressed,
        Event::Wheel { .. } | Event::TouchDown { .. } => true,
        Event::TouchMove { id, .. } => !held.touches.contains(&id),
        _ => false,
    }
}
fn update_held(held: &mut HeldInput, event: &Event) {
    match *event {
        Event::Key { code, pressed } => {
            if pressed {
                held.keys.insert(code);
            } else {
                held.keys.remove(&code);
            }
        }
        Event::Button { code, pressed, .. } => {
            if pressed {
                held.buttons.insert(code);
            } else {
                held.buttons.remove(&code);
            }
        }
        Event::TouchDown { id, .. } => {
            held.touches.insert(id);
        }
        Event::TouchUp { id } | Event::TouchCancel { id } => {
            held.touches.remove(&id);
        }
        _ => {}
    }
}
