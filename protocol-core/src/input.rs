//! Bounded input interpretation for cooperating sources. A trusted adapter must
//! validate the current session lease and admit exact command bytes through the
//! command ledger before dispatch. This module never injects operating-system input.
use crate::commands::Id;
use crate::model::{Affine, Point, Rect};
use std::collections::BTreeSet;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextOwner {
    HostIme,
    ClientText,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum InputError {
    #[error("invalid or unsupported input geometry")]
    Geometry,
    #[error("input outside the authorized region")]
    Outside,
    #[error("input limit exceeded")]
    Limit,
    #[error("stale surface route or presentation")]
    StaleRoute,
    #[error("input target is not authorized, mapped, focused, or permitted by modality")]
    Unauthorized,
    #[error("timestamps must preserve reliable event order")]
    Timestamp,
    #[error("invalid held-input transition or client-generated repeat")]
    Transition,
    #[error("text ownership forbids physical composing keys")]
    TextOwnership,
    #[error("stale editing context or composition")]
    StaleComposition,
    #[error("composition identity retention exhausted")]
    Retention,
    #[error("adapter outcome is unknown; unwind retained input before continuing")]
    OutcomeUnknown,
}

fn finite_point(p: Point) -> bool {
    p.x.is_finite() && p.y.is_finite()
}
fn valid_rect(r: Rect) -> bool {
    finite_point(Point { x: r.x, y: r.y })
        && r.width.is_finite()
        && r.height.is_finite()
        && r.width > 0.0
        && r.height > 0.0
        && (r.x + r.width).is_finite()
        && (r.y + r.height).is_finite()
}
fn contains(r: Rect, p: Point) -> bool {
    p.x >= r.x && p.y >= r.y && p.x < r.x + r.width && p.y < r.y + r.height
}

/// Invert the client's presentation transform. Singular transforms and pixels
/// in letterboxing cannot become authoritative pointer coordinates.
pub fn inverse_pointer(
    client: Point,
    host_to_client: Affine,
    clip: Rect,
    host_bounds: Rect,
) -> Result<Point, InputError> {
    if !finite_point(client)
        || !valid_rect(clip)
        || !valid_rect(host_bounds)
        || [
            host_to_client.xx,
            host_to_client.xy,
            host_to_client.yx,
            host_to_client.yy,
            host_to_client.tx,
            host_to_client.ty,
        ]
        .iter()
        .any(|v| !v.is_finite())
    {
        return Err(InputError::Geometry);
    }
    if !contains(clip, client) {
        return Err(InputError::Outside);
    }
    let determinant = host_to_client.xx * host_to_client.yy - host_to_client.xy * host_to_client.yx;
    if determinant == 0.0 || !determinant.is_finite() {
        return Err(InputError::Geometry);
    }
    let x = client.x - host_to_client.tx;
    let y = client.y - host_to_client.ty;
    let p = Point {
        x: (host_to_client.yy * x - host_to_client.xy * y) / determinant,
        y: (host_to_client.xx * y - host_to_client.yx * x) / determinant,
    };
    if !finite_point(p) {
        return Err(InputError::Geometry);
    }
    if !contains(host_bounds, p) {
        return Err(InputError::Outside);
    }
    Ok(p)
}

/// Map host logical coordinates into the explicitly granted EIS region. Neither
/// rectangle is inferred from framebuffer pixels or display resolution.
pub fn map_granted_region(point: Point, logical: Rect, granted: Rect) -> Result<Point, InputError> {
    if !finite_point(point) || !valid_rect(logical) || !valid_rect(granted) {
        return Err(InputError::Geometry);
    }
    if !contains(logical, point) {
        return Err(InputError::Outside);
    }
    let result = Point {
        x: granted.x + (point.x - logical.x) / logical.width * granted.width,
        y: granted.y + (point.y - logical.y) / logical.height * granted.height,
    };
    if !finite_point(result) || !contains(granted, result) {
        return Err(InputError::Geometry);
    }
    Ok(result)
}

/// Source-issued route evaluated at dispatch. The cooperating source guarantees
/// that hit testing and dispatch use this same topology/focus boundary.
#[derive(Clone, Debug)]
pub struct Route {
    pub surface: u64,
    pub token: Id,
    pub presented_revision: u64,
    pub bounds: Rect,
    pub mapped: bool,
    pub focused: bool,
    pub blocked_by_modal: bool,
    pub authorized: bool,
    pub text_owner: TextOwner,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Motion(Point),
    Button {
        code: u32,
        pressed: bool,
        position: Point,
    },
    /// Linux evdev physical codes; repeats originate at the host, never here.
    Key {
        code: u32,
        pressed: bool,
    },
    Wheel {
        x: f64,
        y: f64,
        position: Point,
    },
    TouchDown {
        id: u64,
        position: Point,
    },
    TouchMove {
        id: u64,
        position: Point,
    },
    TouchUp {
        id: u64,
    },
    TouchCancel {
        id: u64,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct TimedEvent {
    pub timestamp_us: u64,
    pub event: Event,
}
#[derive(Clone, Debug)]
pub struct InputBatch {
    pub surface: u64,
    pub routing_token: Id,
    pub presented_revision: u64,
    pub events: Vec<TimedEvent>,
}
#[derive(Clone, Debug)]
pub struct InputLimits {
    pub max_events: usize,
    pub max_held: usize,
    pub max_coordinate: f64,
}
impl Default for InputLimits {
    fn default() -> Self {
        Self {
            max_events: 256,
            max_held: 128,
            max_coordinate: 1e9,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeldInput {
    pub keys: BTreeSet<u32>,
    pub buttons: BTreeSet<u32>,
    pub touches: BTreeSet<u64>,
}
impl HeldInput {
    fn len(&self) -> usize {
        self.keys.len() + self.buttons.len() + self.touches.len()
    }
}

/// One trusted active lease, seat, device, and captured source surface per router.
/// Destroy/drain this router before changing any of those bindings. No event is
/// coalesced. Full batch validation precedes any external dispatch.
pub struct InputRouter {
    surface: u64,
    limits: InputLimits,
    held: HeldInput,
    timestamp: u64,
    failed: bool,
}
impl InputRouter {
    pub fn new(surface: u64, limits: InputLimits) -> Result<Self, InputError> {
        if surface == 0
            || limits.max_events == 0
            || limits.max_held == 0
            || !limits.max_coordinate.is_finite()
            || limits.max_coordinate <= 0.0
        {
            return Err(InputError::Limit);
        }
        Ok(Self {
            surface,
            limits,
            held: HeldInput::default(),
            timestamp: 0,
            failed: false,
        })
    }
    pub fn held(&self) -> &HeldInput {
        &self.held
    }
    /// Pure full-batch preflight before an adapter starts emulation or sends any
    /// event. The bounded temporary state does not consume held transitions or
    /// timestamp order in this router. Actual dispatch revalidates the batch.
    pub fn validate(&self, route: &Route, batch: &InputBatch) -> Result<(), InputError> {
        let mut temporary = Self {
            surface: self.surface,
            limits: self.limits.clone(),
            held: self.held.clone(),
            timestamp: self.timestamp,
            failed: self.failed,
        };
        temporary.dispatch(route, batch, |_| Ok(()))
    }
    /// Trusted cleanup bypasses stale routes, modality and focus. The adapter
    /// must deliver every release/cancel before another lease receives input.
    pub fn drain_cleanup(&mut self) -> HeldInput {
        self.failed = false;
        std::mem::take(&mut self.held)
    }
    pub fn dispatch<F>(
        &mut self,
        route: &Route,
        batch: &InputBatch,
        mut send: F,
    ) -> Result<(), InputError>
    where
        F: FnMut(&TimedEvent) -> Result<(), ()>,
    {
        if self.failed {
            return Err(InputError::OutcomeUnknown);
        }
        if batch.events.is_empty() || batch.events.len() > self.limits.max_events {
            return Err(InputError::Limit);
        }
        if route.surface != self.surface || batch.surface != self.surface {
            return Err(InputError::Unauthorized);
        }
        if !valid_rect(route.bounds) {
            return Err(InputError::Geometry);
        }
        let mut candidate = self.held.clone();
        let mut timestamp = self.timestamp;
        // Retain every possible press until all callbacks complete, including a
        // press/release pair whose release callback later fails ambiguously.
        let mut possible = self.held.clone();
        for timed in &batch.events {
            if timed.timestamp_us < timestamp {
                return Err(InputError::Timestamp);
            }
            timestamp = timed.timestamp_us;
            let cleanup = match timed.event {
                Event::Key {
                    code,
                    pressed: false,
                } => candidate.keys.contains(&code),
                Event::Button {
                    code,
                    pressed: false,
                    ..
                } => candidate.buttons.contains(&code),
                Event::TouchUp { id } | Event::TouchCancel { id } => {
                    candidate.touches.contains(&id)
                }
                _ => false,
            };
            if !cleanup {
                if !route.authorized || !route.mapped || route.blocked_by_modal {
                    return Err(InputError::Unauthorized);
                }
                if batch.routing_token != route.token
                    || batch.presented_revision == 0
                    || batch.presented_revision != route.presented_revision
                {
                    return Err(InputError::StaleRoute);
                }
            }
            let point = match timed.event {
                Event::Motion(p)
                | Event::Button { position: p, .. }
                | Event::Wheel { position: p, .. }
                | Event::TouchDown { position: p, .. }
                | Event::TouchMove { position: p, .. } => Some(p),
                _ => None,
            };
            if let Some(p) = point {
                if !finite_point(p)
                    || p.x.abs() > self.limits.max_coordinate
                    || p.y.abs() > self.limits.max_coordinate
                {
                    return Err(InputError::Geometry);
                }
                let captured = match timed.event {
                    Event::Motion(_) => !candidate.buttons.is_empty(),
                    Event::TouchMove { id, .. } => candidate.touches.contains(&id),
                    _ => false,
                };
                if !cleanup && !captured && !contains(route.bounds, p) {
                    return Err(InputError::Outside);
                }
            }
            match timed.event {
                Event::Key { code, pressed } => {
                    if code == 0 || code > 767 {
                        return Err(InputError::Transition);
                    }
                    if !cleanup && (!route.focused || route.text_owner != TextOwner::HostIme) {
                        return Err(InputError::TextOwnership);
                    }
                    transition(&mut candidate.keys, code, pressed)?;
                    if pressed {
                        possible.keys.insert(code);
                    }
                }
                Event::Button { code, pressed, .. } => {
                    if code == 0 || code > 32 {
                        return Err(InputError::Transition);
                    }
                    transition(&mut candidate.buttons, code, pressed)?;
                    if pressed {
                        possible.buttons.insert(code);
                    }
                }
                Event::TouchDown { id, .. } => {
                    transition(&mut candidate.touches, id, true)?;
                    possible.touches.insert(id);
                }
                Event::TouchUp { id } | Event::TouchCancel { id } => {
                    transition(&mut candidate.touches, id, false)?
                }
                Event::TouchMove { id, .. } => {
                    if !candidate.touches.contains(&id) {
                        return Err(InputError::Transition);
                    }
                }
                Event::Wheel { x, y, .. } => {
                    if !x.is_finite()
                        || !y.is_finite()
                        || x.abs() > self.limits.max_coordinate
                        || y.abs() > self.limits.max_coordinate
                    {
                        return Err(InputError::Geometry);
                    }
                }
                Event::Motion(_) => {}
            }
            if candidate.len() > self.limits.max_held || possible.len() > self.limits.max_held {
                return Err(InputError::Limit);
            }
        }
        // Reserve conservative cleanup ownership before calling external code.
        self.held = possible;
        self.failed = true;
        self.timestamp = timestamp;
        for timed in &batch.events {
            send(timed).map_err(|_| InputError::OutcomeUnknown)?;
        }
        self.held = candidate;
        self.failed = false;
        Ok(())
    }
}
fn transition<T: Ord>(set: &mut BTreeSet<T>, key: T, pressed: bool) -> Result<(), InputError> {
    let changed = if pressed {
        set.insert(key)
    } else {
        set.remove(&key)
    };
    if changed {
        Ok(())
    } else {
        Err(InputError::Transition)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditingContext {
    pub target: u64,
    pub focus_generation: u64,
    pub text_version: u64,
    pub scalar_count: u64,
    pub owner: TextOwner,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScalarRange {
    pub start: u64,
    pub end: u64,
}
fn range_valid(range: ScalarRange, count: u64) -> bool {
    range.start <= range.end && range.end <= count
}
struct Composition {
    id: Id,
    context: EditingContext,
    range: ScalarRange,
    preedit: String,
    selection: ScalarRange,
}
/// This value contains private text and deliberately has no Debug implementation.
pub struct TextEdit {
    pub target: u64,
    pub focus_generation: u64,
    pub text_version: u64,
    pub range: ScalarRange,
    pub text: String,
}
/// A single focused source context. Preedit is transient and never mutates the
/// document. Completion yields an edit that the source must atomically validate
/// and apply; no APPLIED claim is made by this state machine.
pub struct CompositionMachine {
    context: Option<EditingContext>,
    active: Option<Composition>,
    used: BTreeSet<Id>,
    max_sessions: usize,
    max_bytes: usize,
}
impl CompositionMachine {
    pub fn new(max_sessions: usize, max_bytes: usize) -> Result<Self, InputError> {
        if max_sessions == 0 || max_bytes == 0 {
            return Err(InputError::Limit);
        }
        Ok(Self {
            context: None,
            active: None,
            used: BTreeSet::new(),
            max_sessions,
            max_bytes,
        })
    }
    /// Trusted focus/source change, node deletion, or text-owner renegotiation.
    /// Returns a cancelled composition identity for the adapter to notify.
    pub fn set_context(&mut self, context: Option<EditingContext>) -> Option<Id> {
        let cancelled = if self.context != context {
            self.cancel()
        } else {
            None
        };
        self.context = context;
        cancelled
    }
    pub fn cancel(&mut self) -> Option<Id> {
        self.active.take().map(|v| v.id)
    }
    pub fn active_id(&self) -> Option<Id> {
        self.active.as_ref().map(|v| v.id)
    }
    pub fn active_range(&self) -> Option<ScalarRange> {
        self.active.as_ref().map(|v| v.range)
    }
    pub fn preedit(&self) -> Option<&str> {
        self.active.as_ref().map(|v| v.preedit.as_str())
    }
    pub fn preedit_selection(&self) -> Option<ScalarRange> {
        self.active.as_ref().map(|v| v.selection)
    }
    /// Pure preflight; does not retire an identity or mutate transient text.
    pub fn validate_begin(
        &self,
        id: Id,
        focus: u64,
        version: u64,
        range: ScalarRange,
    ) -> Result<(), InputError> {
        let context = self.context.as_ref().ok_or(InputError::StaleComposition)?;
        if context.owner != TextOwner::ClientText {
            return Err(InputError::TextOwnership);
        }
        if id == [0; 16]
            || context.target == 0
            || focus == 0
            || version == 0
            || context.focus_generation != focus
            || context.text_version != version
            || self.active.is_some()
            || self.used.contains(&id)
        {
            return Err(InputError::StaleComposition);
        }
        if !range_valid(range, context.scalar_count) {
            return Err(InputError::Outside);
        }
        if self.used.len() >= self.max_sessions {
            return Err(InputError::Retention);
        }
        Ok(())
    }
    pub fn begin(
        &mut self,
        id: Id,
        focus: u64,
        version: u64,
        range: ScalarRange,
    ) -> Result<(), InputError> {
        self.validate_begin(id, focus, version, range)?;
        let context = self.context.as_ref().unwrap();
        self.used.insert(id);
        self.active = Some(Composition {
            id,
            context: context.clone(),
            range,
            preedit: String::new(),
            selection: ScalarRange { start: 0, end: 0 },
        });
        Ok(())
    }
    pub fn validate_update(
        &self,
        id: Id,
        text: &str,
        selection: ScalarRange,
    ) -> Result<(), InputError> {
        if text.len() > self.max_bytes {
            return Err(InputError::Limit);
        }
        if !range_valid(selection, text.chars().count() as u64) {
            return Err(InputError::Outside);
        }
        self.active
            .as_ref()
            .filter(|v| v.id == id)
            .ok_or(InputError::StaleComposition)?;
        Ok(())
    }
    pub fn update(&mut self, id: Id, text: &str, selection: ScalarRange) -> Result<(), InputError> {
        self.validate_update(id, text, selection)?;
        let active = self.active.as_mut().unwrap();
        active.preedit = text.to_owned();
        active.selection = selection;
        Ok(())
    }
    pub fn validate_commit(
        &self,
        id: Id,
        focus: u64,
        version: u64,
        text: &str,
    ) -> Result<(), InputError> {
        if text.len() > self.max_bytes {
            return Err(InputError::Limit);
        }
        let active = self
            .active
            .as_ref()
            .filter(|v| {
                v.id == id
                    && v.context.focus_generation == focus
                    && v.context.text_version == version
            })
            .ok_or(InputError::StaleComposition)?;
        if self.context.as_ref() != Some(&active.context) {
            return Err(InputError::StaleComposition);
        }
        Ok(())
    }
    pub fn commit(
        &mut self,
        id: Id,
        focus: u64,
        version: u64,
        text: &str,
    ) -> Result<TextEdit, InputError> {
        self.validate_commit(id, focus, version, text)?;
        let active = self.active.as_ref().unwrap();
        let edit = TextEdit {
            target: active.context.target,
            focus_generation: focus,
            text_version: version,
            range: active.range,
            text: text.to_owned(),
        };
        self.active = None;
        Ok(edit)
    }
}
