//! Generic cooperating-source controls discovered from the authoritative view.
//! Canvas strokes retain every sample and are submitted only on release.
use crate::{
    interaction::{self, CommandBinding, InteractionError},
    Viewport,
};
use rui_protocol_core::{
    model::*,
    resources::{MemoryBudget, MemoryReservation},
    wire::proto as p,
};
use sha2::{Digest, Sha256};
use thiserror::Error;
pub const MAX_STROKE_POINTS: usize = 64;
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SourceControlError {
    #[error("stroke or source-control memory limit reached")]
    Limit,
    #[error("source view or input route changed")]
    Stale,
    #[error("the host has not advertised one supported device scope")]
    Device,
    #[error("stroke geometry or event order is invalid")]
    Geometry,
    #[error("no enabled source canvas or resizable surface is available")]
    Target,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerSample {
    pub timestamp_us: u64,
    pub point: Point,
}
pub struct PointerStroke {
    revision: u64,
    viewport: Viewport,
    points: Vec<PointerSample>,
    complete: bool,
    failed: bool,
    memory: MemoryReservation,
}
pub struct DeviceScope<'a> {
    pub seat: &'a str,
    pub device: &'a str,
    pub key_namespace: &'a str,
    pub input_mode: &'a str,
    pub text_owner: &'a str,
}
pub struct PreparedRaw {
    wire: p::RawInputBatch,
    _memory: MemoryReservation,
}
impl PreparedRaw {
    pub fn wire(&self) -> &p::RawInputBatch {
        &self.wire
    }
    pub fn event_digest(&self) -> Option<[u8; 32]> {
        use rui_protocol_core::input::{Event, TimedEvent};
        let mut digest = Sha256::new();
        for e in &self.wire.events {
            let event = match e.event.as_ref()? {
                p::raw_event::Event::Motion(p) => Event::Motion(Point { x: p.x, y: p.y }),
                p::raw_event::Event::Button(b) => {
                    let p = b.position.as_ref()?;
                    Event::Button {
                        code: b.code,
                        pressed: b.pressed,
                        position: Point { x: p.x, y: p.y },
                    }
                }
                _ => return None,
            };
            feed_pointer(
                &mut digest,
                TimedEvent {
                    timestamp_us: e.timestamp_us,
                    event,
                },
            )?;
        }
        Some(digest.finalize().into())
    }
}
fn feed_pointer(digest: &mut Sha256, event: rui_protocol_core::input::TimedEvent) -> Option<()> {
    use rui_protocol_core::input::Event;
    digest.update(event.timestamp_us.to_be_bytes());
    let point = match event.event {
        Event::Motion(p) => {
            digest.update([1]);
            p
        }
        Event::Button {
            code,
            pressed,
            position,
        } => {
            digest.update([2]);
            digest.update(code.to_be_bytes());
            digest.update([pressed as u8]);
            position
        }
        _ => return None,
    };
    digest.update(point.x.to_bits().to_be_bytes());
    digest.update(point.y.to_bits().to_be_bytes());
    Some(())
}
/// Allocation-free local test fingerprint including every event's order,
/// timestamp, kind, press state and exact authoritative point.
pub fn pointer_event_digest(
    events: impl Iterator<Item = rui_protocol_core::input::TimedEvent>,
) -> Option<[u8; 32]> {
    let mut digest = Sha256::new();
    for event in events {
        feed_pointer(&mut digest, event)?;
    }
    Some(digest.finalize().into())
}
fn geometry(view: Viewport, p: Point) -> bool {
    p.x.is_finite()
        && p.y.is_finite()
        && p.x >= 0.0
        && p.y >= 0.0
        && p.x < view.width as f64
        && p.y < view.height as f64
        && view.scale.is_finite()
        && view.scale > 0.0
}
fn contains(r: Rect, p: Point) -> bool {
    p.x >= r.x && p.y >= r.y && p.x < r.x + r.width && p.y < r.y + r.height
}
fn header(binding: CommandBinding, revision: u64) -> Result<p::CommandHeader, SourceControlError> {
    if binding.lease == [0; 16] || binding.generation == 0 || binding.sequence == 0 || revision == 0
    {
        return Err(SourceControlError::Stale);
    }
    Ok(p::CommandHeader {
        epoch: binding.epoch.to_vec(),
        view_id: binding.view.to_vec(),
        client_instance_id: binding.client.to_vec(),
        command_seq: binding.sequence,
        control_lease_id: binding.lease.to_vec(),
        control_generation: binding.generation,
        observed_view_revision: revision,
    })
}
impl PointerStroke {
    pub fn begin(
        revision: u64,
        viewport: Viewport,
        sample: PointerSample,
        budget: &MemoryBudget,
    ) -> Result<Self, SourceControlError> {
        if revision == 0 || !geometry(viewport, sample.point) {
            return Err(SourceControlError::Geometry);
        }
        // Includes capture storage and the later bounded protobuf event graph.
        let memory = budget
            .reserve(MAX_STROKE_POINTS * 512 + 8192)
            .map_err(|_| SourceControlError::Limit)?;
        let mut points = Vec::new();
        points
            .try_reserve_exact(MAX_STROKE_POINTS)
            .map_err(|_| SourceControlError::Limit)?;
        points.push(sample);
        Ok(Self {
            revision,
            viewport,
            points,
            complete: false,
            failed: false,
            memory,
        })
    }
    pub fn push(&mut self, sample: PointerSample) -> Result<(), SourceControlError> {
        let error = if self.failed || self.complete || self.points.len() == MAX_STROKE_POINTS {
            Some(SourceControlError::Limit)
        } else if !geometry(self.viewport, sample.point)
            || sample.timestamp_us < self.points.last().unwrap().timestamp_us
        {
            Some(SourceControlError::Geometry)
        } else {
            None
        };
        if let Some(error) = error {
            self.failed = true;
            return Err(error);
        }
        self.points.push(sample);
        Ok(())
    }
    pub fn finish(&mut self, sample: PointerSample) -> Result<(), SourceControlError> {
        self.push(sample)?;
        self.complete = true;
        Ok(())
    }
    pub fn samples(&self) -> &[PointerSample] {
        &self.points
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }
    pub fn matches_viewport(&self, view: Viewport) -> bool {
        (
            self.viewport.width,
            self.viewport.height,
            self.viewport.origin,
            self.viewport.scale,
        ) == (view.width, view.height, view.origin, view.scale)
    }
    pub fn into_command(
        self,
        state: &State,
        binding: CommandBinding,
        device: DeviceScope<'_>,
    ) -> Result<PreparedRaw, SourceControlError> {
        if !self.complete || self.failed || state.revision != self.revision {
            return Err(SourceControlError::Stale);
        }
        if device.seat.is_empty()
            || device.device.is_empty()
            || device.seat.len() > 64
            || device.device.len() > 64
            || device.key_namespace != "linux.evdev"
            || !matches!(
                (device.input_mode, device.text_owner),
                ("raw.pointer.v1", "CLIENT_TEXT") | ("raw.host.v1", "HOST_PHYSICAL")
            )
        {
            return Err(SourceControlError::Device);
        }
        let target =
            interaction::canvas_hit_test(state, self.revision, self.viewport, self.points[0].point)
                .map_err(|_| SourceControlError::Target)?;
        let Some(Record::Surface(surface)) = state.records.get(&target.surface) else {
            return Err(SourceControlError::Target);
        };
        let token: [u8; 16] = surface
            .routing_token
            .as_slice()
            .try_into()
            .map_err(|_| SourceControlError::Stale)?;
        if token == [0; 16] {
            return Err(SourceControlError::Stale);
        }
        let mut events = Vec::new();
        events
            .try_reserve_exact(self.points.len())
            .map_err(|_| SourceControlError::Limit)?;
        for (index, sample) in self.points.iter().enumerate() {
            let candidate =
                interaction::canvas_hit_test(state, self.revision, self.viewport, sample.point)
                    .map_err(|_| SourceControlError::Target)?;
            if candidate != target {
                return Err(SourceControlError::Target);
            }
            let point = interaction::local(
                surface,
                Point {
                    x: self.viewport.origin.x + sample.point.x / self.viewport.scale,
                    y: self.viewport.origin.y + sample.point.y / self.viewport.scale,
                },
            )
            .ok_or(SourceControlError::Geometry)?;
            if !surface.input_region.iter().any(|r| contains(*r, point)) {
                return Err(SourceControlError::Geometry);
            }
            let position = p::Point {
                x: point.x,
                y: point.y,
            };
            let event = if index == 0 || index + 1 == self.points.len() {
                p::raw_event::Event::Button(p::ButtonEvent {
                    code: 1,
                    pressed: index == 0,
                    position: Some(position),
                })
            } else {
                p::raw_event::Event::Motion(position)
            };
            events.push(p::RawEvent {
                timestamp_us: sample.timestamp_us,
                event: Some(event),
            });
        }
        Ok(PreparedRaw {
            wire: p::RawInputBatch {
                header: Some(header(binding, self.revision)?),
                surface_id: target.surface,
                routing_token: token.to_vec(),
                presented_revision: self.revision,
                seat_id: device.seat.into(),
                device_id: device.device.into(),
                key_namespace: device.key_namespace.into(),
                events,
            },
            _memory: self.memory,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizeTarget {
    pub surface: u64,
    pub version: u64,
    pub width: f64,
    pub height: f64,
}
pub fn resize_target(state: &State) -> Result<ResizeTarget, SourceControlError> {
    let surface = state
        .records
        .values()
        .find_map(|record| match record {
            Record::Session(s) => s.focus_surface,
            _ => None,
        })
        .and_then(|reference| state.records.get(&reference.id))
        .and_then(|record| match record {
            Record::Surface(surface) => Some(surface),
            _ => None,
        })
        .ok_or(SourceControlError::Target)?;
    if !surface.mapped||surface.role!=SurfaceRole::Toplevel||state.records.values().any(|record|matches!(record,Record::Surface(modal) if modal.mapped&&modal.modal_for==Some(ObjectRef{kind:ObjectKind::Surface,id:surface.id}))) {return Err(SourceControlError::Target);}
    Ok(ResizeTarget {
        surface: surface.id,
        version: surface.version,
        width: surface.content_size.width,
        height: surface.content_size.height,
    })
}
pub struct PreparedResize {
    wire: p::ResizeRequest,
    _memory: MemoryReservation,
}
impl PreparedResize {
    pub fn wire(&self) -> &p::ResizeRequest {
        &self.wire
    }
}
pub fn resize_command(
    state: &State,
    captured: ResizeTarget,
    width: f64,
    height: f64,
    binding: CommandBinding,
    budget: &MemoryBudget,
) -> Result<PreparedResize, SourceControlError> {
    if resize_target(state)? != captured {
        return Err(SourceControlError::Stale);
    }
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err(SourceControlError::Geometry);
    }
    let memory = budget
        .reserve(8192)
        .map_err(|_| SourceControlError::Limit)?;
    Ok(PreparedResize {
        wire: p::ResizeRequest {
            header: Some(header(binding, state.revision)?),
            surface_id: captured.surface,
            surface_version: captured.version,
            width,
            height,
        },
        _memory: memory,
    })
}
impl From<InteractionError> for SourceControlError {
    fn from(_: InteractionError) -> Self {
        Self::Target
    }
}
