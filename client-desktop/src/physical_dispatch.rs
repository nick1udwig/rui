//! Physical input binds the actual painted frame's immutable routing metadata.
//! The current replica may advance only while that exact target, geometry,
//! focus and route remain valid. Admission remains in the common host ledger.
use crate::{
    interaction::{self, CommandBinding},
    physical_input::{PhysicalInputError, PhysicalKey, PhysicalKeyOwner},
    source_controls::DeviceScope,
    Viewport,
};
use rui_protocol_core::{
    input::{Event, TimedEvent},
    model::*,
    resources::{MemoryBudget, MemoryReservation},
    wire::proto as p,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalControl {
    pub connection_generation: u64,
    pub grant_serial: u64,
}
const MAX_SURFACES: usize = 128;
const MAX_INPUT_REGIONS: usize = 64;
struct PaintedSurface {
    id: u64,
    token: [u8; 16],
    role: SurfaceRole,
    parent: Option<ObjectRef>,
    desktop_rect: Rect,
    content_size: Size,
    transform: Affine,
    clip: Option<Rect>,
    stacking_order: i64,
    mapped: bool,
    complete: bool,
    focused: bool,
    modal_for: Option<ObjectRef>,
    input_region: Vec<Rect>,
}
impl PaintedSurface {
    fn owner(&self, current: PhysicalControl) -> Option<PhysicalKeyOwner> {
        let owner = PhysicalKeyOwner {
            connection_generation: current.connection_generation,
            grant_serial: current.grant_serial,
            surface: self.id,
            routing_token: self.token,
        };
        (owner.valid() && self.mapped && self.complete).then_some(owner)
    }
    fn unchanged(&self, s: &Surface) -> bool {
        self.id == s.id
            && self.token.as_slice() == s.routing_token
            && self.role == s.role
            && self.parent == s.parent
            && self.desktop_rect == s.desktop_rect
            && self.content_size == s.content_size
            && self.transform == s.transform
            && self.clip == s.clip
            && self.stacking_order == s.stacking_order
            && self.mapped == s.mapped
            && self.complete == (s.visual_coverage == VisualCoverage::Complete)
            && self.focused == s.focused
            && self.modal_for == s.modal_for
            && self.input_region == s.input_region
    }
    fn local(&self, p: Point) -> Option<Point> {
        local(self.desktop_rect, self.transform, p)
    }
    fn hit(&self, p: Point) -> bool {
        self.local(p).is_some_and(|p| {
            contains(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: self.content_size.width,
                    height: self.content_size.height,
                },
                p,
            ) && self.clip.is_none_or(|c| contains(c, p))
                && self.input_region.iter().any(|r| contains(*r, p))
        })
    }
}
/// Shared-budget charged, immutable routing only; no text, scene bodies or
/// resource contents. GTK retains it with the corresponding painted frame.
/// Local input supports at most128 surfaces with64 input rectangles each;
/// larger views remain displayable but local raw capture is unavailable.
pub struct PresentedRouting {
    revision: u64,
    canvas: Rect,
    focus_surface: Option<ObjectRef>,
    focus_generation: u64,
    surfaces: Vec<PaintedSurface>,
    _memory: MemoryReservation,
}
impl PresentedRouting {
    pub fn capture(state: &State, budget: &MemoryBudget) -> Result<Arc<Self>, PhysicalInputError> {
        let session = state
            .records
            .values()
            .find_map(|r| match r {
                Record::Session(s) => Some(s),
                _ => None,
            })
            .ok_or(PhysicalInputError::OwnerChanged)?;
        if state.revision == 0 {
            return Err(PhysicalInputError::OwnerChanged);
        }
        let mut count = 0usize;
        let mut regions = 0usize;
        for record in state.records.values() {
            if let Record::Surface(surface) = record {
                count += 1;
                if count > MAX_SURFACES || surface.input_region.len() > MAX_INPUT_REGIONS {
                    return Err(PhysicalInputError::Limit);
                }
                regions = regions
                    .checked_add(surface.input_region.len())
                    .ok_or(PhysicalInputError::Limit)?;
            }
        }
        let charge = count
            .checked_mul(std::mem::size_of::<PaintedSurface>())
            .and_then(|n| n.checked_add(regions * std::mem::size_of::<Rect>()))
            .and_then(|n| n.checked_add(4096))
            .ok_or(PhysicalInputError::Limit)?;
        let memory = budget
            .reserve(charge)
            .map_err(|_| PhysicalInputError::Limit)?;
        let mut surfaces = Vec::new();
        surfaces
            .try_reserve_exact(count)
            .map_err(|_| PhysicalInputError::Limit)?;
        for record in state.records.values() {
            if let Record::Surface(s) = record {
                let mut input_region = Vec::new();
                input_region
                    .try_reserve_exact(s.input_region.len())
                    .map_err(|_| PhysicalInputError::Limit)?;
                input_region.extend_from_slice(&s.input_region);
                surfaces.push(PaintedSurface {
                    id: s.id,
                    token: s.routing_token.as_slice().try_into().unwrap_or([0; 16]),
                    role: s.role,
                    parent: s.parent,
                    desktop_rect: s.desktop_rect,
                    content_size: s.content_size,
                    transform: s.transform,
                    clip: s.clip,
                    stacking_order: s.stacking_order,
                    mapped: s.mapped,
                    complete: s.visual_coverage == VisualCoverage::Complete,
                    focused: s.focused,
                    modal_for: s.modal_for,
                    input_region,
                });
            }
        }
        Ok(Arc::new(Self {
            revision: state.revision,
            canvas: session.canvas,
            focus_surface: session.focus_surface,
            focus_generation: session.focus_generation,
            surfaces,
            _memory: memory,
        }))
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    fn surface(&self, id: u64) -> Option<&PaintedSurface> {
        self.surfaces.iter().find(|s| s.id == id)
    }
    fn blocked(&self, id: u64) -> bool {
        self.surfaces.iter().any(|s| {
            s.mapped
                && s.modal_for
                    == Some(ObjectRef {
                        kind: ObjectKind::Surface,
                        id,
                    })
        })
    }
    pub fn keyboard_owner(&self, current: PhysicalControl) -> Option<PhysicalKeyOwner> {
        let surface = self.surface(self.focus_surface?.id)?;
        (surface.focused && !self.blocked(surface.id))
            .then(|| surface.owner(current))
            .flatten()
    }
    pub fn pointer_owner(
        &self,
        current: PhysicalControl,
        viewport: Viewport,
        point: Point,
        capture: Option<PhysicalKeyOwner>,
    ) -> Option<PhysicalKeyOwner> {
        let p = desktop(viewport, point)?;
        if !contains(self.canvas, p) {
            return None;
        }
        let surface = if let Some(capture) = capture {
            let surface = self.surface(capture.surface)?;
            (surface.owner(current) == Some(capture)).then_some(surface)?
        } else {
            self.surfaces
                .iter()
                .filter(|s| s.mapped && s.hit(p))
                .max_by_key(|s| (s.stacking_order, s.id))?
        };
        (!self.blocked(surface.id))
            .then(|| surface.owner(current))
            .flatten()
    }
    fn route_unchanged(&self, state: &State, captured: PhysicalKeyOwner) -> bool {
        matches!((self.surface(captured.surface),state.records.get(&captured.surface)),(Some(old),Some(Record::Surface(s))) if old.unchanged(s))
            && pointer_owner_is_current(state, captured)
    }
    fn focus_unchanged(&self, state: &State) -> bool {
        state
            .records
            .values()
            .find_map(|r| match r {
                Record::Session(s) => Some(s),
                _ => None,
            })
            .is_some_and(|s| {
                s.focus_surface == self.focus_surface && s.focus_generation == self.focus_generation
            })
    }
}

pub struct CapturedPhysicalInput {
    pub control: PhysicalControl,
    pub observed_revision: u64,
    viewport: Viewport,
    owner: PhysicalKeyOwner,
    keyboard: bool,
    event: TimedEvent,
    routing: Arc<PresentedRouting>,
    _memory: MemoryReservation,
}
pub struct PreparedPhysicalInput {
    pub wire: p::RawInputBatch,
    pub owner: PhysicalKeyOwner,
    pub event: Event,
    _memory: MemoryReservation,
}
fn control(owner: PhysicalKeyOwner) -> PhysicalControl {
    PhysicalControl {
        connection_generation: owner.connection_generation,
        grant_serial: owner.grant_serial,
    }
}
fn owner(surface: &Surface, current: PhysicalControl) -> Option<PhysicalKeyOwner> {
    let token = surface.routing_token.as_slice().try_into().ok()?;
    let owner = PhysicalKeyOwner {
        connection_generation: current.connection_generation,
        grant_serial: current.grant_serial,
        surface: surface.id,
        routing_token: token,
    };
    (owner.valid() && surface.mapped && surface.visual_coverage == VisualCoverage::Complete)
        .then_some(owner)
}
pub fn pointer_owner_is_current(state: &State, captured: PhysicalKeyOwner) -> bool {
    matches!(state.records.get(&captured.surface),Some(Record::Surface(s)) if owner(s,control(captured))==Some(captured))
        && !blocked_by_modal(state, captured.surface)
}
fn blocked_by_modal(state: &State, id: u64) -> bool {
    state.records.values().any(|r|matches!(r,Record::Surface(s) if s.mapped&&s.modal_for==Some(ObjectRef{kind:ObjectKind::Surface,id})))
}
pub fn keyboard_owner(state: &State, current: PhysicalControl) -> Option<PhysicalKeyOwner> {
    let session = state.records.values().find_map(|r| match r {
        Record::Session(s) => Some(s),
        _ => None,
    })?;
    let Record::Surface(surface) = state.records.get(&session.focus_surface?.id)? else {
        return None;
    };
    (surface.focused && !blocked_by_modal(state, surface.id))
        .then(|| owner(surface, current))
        .flatten()
}
fn contains(r: Rect, p: Point) -> bool {
    p.x >= r.x && p.y >= r.y && p.x < r.x + r.width && p.y < r.y + r.height
}
fn local(rect: Rect, m: Affine, p: Point) -> Option<Point> {
    let determinant = m.xx * m.yy - m.xy * m.yx;
    if determinant == 0.0 || !determinant.is_finite() {
        return None;
    }
    let x = p.x - rect.x - m.tx;
    let y = p.y - rect.y - m.ty;
    let p = Point {
        x: (m.yy * x - m.xy * y) / determinant,
        y: (m.xx * y - m.yx * x) / determinant,
    };
    (p.x.is_finite() && p.y.is_finite()).then_some(p)
}
fn desktop(viewport: Viewport, p: Point) -> Option<Point> {
    if !p.x.is_finite()
        || !p.y.is_finite()
        || p.x < 0.0
        || p.y < 0.0
        || p.x >= viewport.width as f64
        || p.y >= viewport.height as f64
        || !viewport.scale.is_finite()
        || viewport.scale <= 0.0
    {
        return None;
    }
    let p = Point {
        x: viewport.origin.x + p.x / viewport.scale,
        y: viewport.origin.y + p.y / viewport.scale,
    };
    (p.x.is_finite() && p.y.is_finite()).then_some(p)
}
fn point(event: &Event) -> Option<Point> {
    match event {
        Event::Motion(p) | Event::Button { position: p, .. } | Event::Wheel { position: p, .. } => {
            Some(*p)
        }
        _ => None,
    }
}
impl CapturedPhysicalInput {
    pub fn key(
        routing: Arc<PresentedRouting>,
        current: PhysicalControl,
        viewport: Viewport,
        timestamp_us: u64,
        key: PhysicalKey,
        budget: &MemoryBudget,
    ) -> Result<Self, PhysicalInputError> {
        let owner = routing
            .keyboard_owner(current)
            .ok_or(PhysicalInputError::OwnerChanged)?;
        if key.code == 0 || key.code > 767 {
            return Err(PhysicalInputError::KeyMapping);
        }
        let memory = budget
            .reserve(4096)
            .map_err(|_| PhysicalInputError::Limit)?;
        Ok(Self {
            control: current,
            observed_revision: routing.revision,
            viewport,
            owner,
            keyboard: true,
            event: TimedEvent {
                timestamp_us,
                event: Event::Key {
                    code: key.code,
                    pressed: key.pressed,
                },
            },
            routing,
            _memory: memory,
        })
    }
    pub fn pointer(
        routing: Arc<PresentedRouting>,
        current: PhysicalControl,
        viewport: Viewport,
        event: TimedEvent,
        budget: &MemoryBudget,
        capture: Option<PhysicalKeyOwner>,
    ) -> Result<Self, PhysicalInputError> {
        let p = point(&event.event).ok_or(PhysicalInputError::KeyMapping)?;
        let owner = routing
            .pointer_owner(current, viewport, p, capture)
            .ok_or(PhysicalInputError::OwnerChanged)?;
        match event.event {
            Event::Button { code, .. } if code == 0 || code > 32 => {
                return Err(PhysicalInputError::KeyMapping)
            }
            Event::Wheel { x, y, .. } if !x.is_finite() || !y.is_finite() => {
                return Err(PhysicalInputError::KeyMapping)
            }
            _ => {}
        }
        let memory = budget
            .reserve(4096)
            .map_err(|_| PhysicalInputError::Limit)?;
        Ok(Self {
            control: current,
            observed_revision: routing.revision,
            viewport,
            owner,
            keyboard: false,
            event,
            routing,
            _memory: memory,
        })
    }
    pub fn captured_owner(&self) -> PhysicalKeyOwner {
        self.owner
    }
    pub fn prepare(
        self,
        state: &State,
        binding: CommandBinding,
        presented_revision: u64,
        current: PhysicalControl,
        pointer_capture: Option<PhysicalKeyOwner>,
        device: DeviceScope<'_>,
    ) -> Result<PreparedPhysicalInput, PhysicalInputError> {
        if current != self.control
            || presented_revision == 0
            || state.revision < presented_revision
            || self.observed_revision > presented_revision
            || binding.lease == [0; 16]
            || binding.generation == 0
            || binding.sequence == 0
            || !self.routing.route_unchanged(state, self.owner)
        {
            return Err(PhysicalInputError::OwnerChanged);
        }
        if device.input_mode != "raw.host.v1"
            || device.text_owner != "HOST_PHYSICAL"
            || device.key_namespace != "linux.evdev"
            || device.seat.is_empty()
            || device.device.is_empty()
            || device.seat.len() > 64
            || device.device.len() > 64
        {
            return Err(PhysicalInputError::KeyMapping);
        }
        let source_event = if self.keyboard {
            if !self.routing.focus_unchanged(state)
                || keyboard_owner(state, current) != Some(self.owner)
            {
                return Err(PhysicalInputError::OwnerChanged);
            }
            self.event.event
        } else {
            let p = desktop(
                self.viewport,
                point(&self.event.event).ok_or(PhysicalInputError::KeyMapping)?,
            )
            .ok_or(PhysicalInputError::KeyMapping)?;
            let session = state
                .records
                .values()
                .find_map(|r| match r {
                    Record::Session(s) => Some(s),
                    _ => None,
                })
                .ok_or(PhysicalInputError::OwnerChanged)?;
            if session.canvas != self.routing.canvas || !contains(session.canvas, p) {
                return Err(PhysicalInputError::OwnerChanged);
            }
            if let Some(captured) = pointer_capture {
                if captured != self.owner {
                    return Err(PhysicalInputError::OwnerChanged);
                }
            } else {
                let target = state
                    .records
                    .values()
                    .filter_map(|r| match r {
                        Record::Surface(s) if s.mapped => Some(s),
                        _ => None,
                    })
                    .filter(|s| {
                        interaction::local(s, p).is_some_and(|p| {
                            contains(
                                Rect {
                                    x: 0.0,
                                    y: 0.0,
                                    width: s.content_size.width,
                                    height: s.content_size.height,
                                },
                                p,
                            ) && s.clip.is_none_or(|c| contains(c, p))
                                && s.input_region.iter().any(|r| contains(*r, p))
                        })
                    })
                    .max_by_key(|s| (s.stacking_order, s.id))
                    .ok_or(PhysicalInputError::OwnerChanged)?;
                if owner(target, current) != Some(self.owner) {
                    return Err(PhysicalInputError::OwnerChanged);
                }
            }
            let Some(Record::Surface(surface)) = state.records.get(&self.owner.surface) else {
                return Err(PhysicalInputError::OwnerChanged);
            };
            let position =
                interaction::local(surface, p).ok_or(PhysicalInputError::OwnerChanged)?;
            match self.event.event {
                Event::Motion(_) => Event::Motion(position),
                Event::Button { code, pressed, .. } => Event::Button {
                    code,
                    pressed,
                    position,
                },
                Event::Wheel { x, y, .. } => Event::Wheel { x, y, position },
                _ => return Err(PhysicalInputError::KeyMapping),
            }
        };
        let position = |p: Point| Some(p::Point { x: p.x, y: p.y });
        let event = match source_event {
            Event::Key { code, pressed } => p::raw_event::Event::Key(p::KeyEvent { code, pressed }),
            Event::Motion(p) => p::raw_event::Event::Motion(p::Point { x: p.x, y: p.y }),
            Event::Button {
                code,
                pressed,
                position: p,
            } => p::raw_event::Event::Button(p::ButtonEvent {
                code,
                pressed,
                position: position(p),
            }),
            Event::Wheel { x, y, position: p } => p::raw_event::Event::Wheel(p::WheelEvent {
                x,
                y,
                position: position(p),
            }),
            _ => return Err(PhysicalInputError::KeyMapping),
        };
        let wire = p::RawInputBatch {
            header: Some(p::CommandHeader {
                epoch: binding.epoch.to_vec(),
                view_id: binding.view.to_vec(),
                client_instance_id: binding.client.to_vec(),
                command_seq: binding.sequence,
                control_lease_id: binding.lease.to_vec(),
                control_generation: binding.generation,
                observed_view_revision: presented_revision,
            }),
            surface_id: self.owner.surface,
            routing_token: self.owner.routing_token.to_vec(),
            presented_revision,
            seat_id: device.seat.into(),
            device_id: device.device.into(),
            key_namespace: device.key_namespace.into(),
            events: vec![p::RawEvent {
                timestamp_us: self.event.timestamp_us,
                event: Some(event),
            }],
        };
        Ok(PreparedPhysicalInput {
            wire,
            owner: self.owner,
            event: source_event,
            _memory: self._memory,
        })
    }
}
