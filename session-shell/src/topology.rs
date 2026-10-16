//! Authoritative topology for the built-in isolated vector input laboratory.
//! Absolute placement is explicit; parent links supply topology, never offsets.
use crate::{RegionGrant, ShellError};
use rui_protocol_core::{
    input::{Route, TextOwner},
    model::{Affine, Color, CursorShape, Limits, Point, Rect, Size, State, SurfaceRole},
    resources::{MemoryBudget, MemoryReservation},
    wire::proto as pb,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct WindowSpec {
    pub role: SurfaceRole,
    pub parent: Option<u64>,
    pub modal_for: Option<u64>,
    pub desktop_rect: Rect,
    pub content_size: Size,
    pub transform: Affine,
    pub stacking_order: i64,
    pub color: Color,
}
#[derive(Clone, Debug)]
struct Window {
    id: u64,
    version: u64,
    route_clock: u64,
    spec: WindowSpec,
    mapped: bool,
    text: String,
    activity: u64,
}
impl Window {
    fn root(&self) -> u64 {
        self.id + 1
    }
    fn background(&self) -> u64 {
        self.id + 2
    }
    fn indicator(&self) -> u64 {
        self.id + 3
    }
    fn semantic(&self) -> u64 {
        self.id + 4
    }
    fn binding(&self) -> u64 {
        self.id + 5
    }
    fn ids(&self) -> std::ops::Range<u64> {
        self.id..self.id + 6
    }
    fn bounds(&self) -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            width: self.spec.content_size.width,
            height: self.spec.content_size.height,
        }
    }
    fn token(&self) -> [u8; 16] {
        let mut result = [0; 16];
        result[..8].copy_from_slice(&self.id.to_be_bytes());
        result[8..].copy_from_slice(&self.route_clock.to_be_bytes());
        result
    }
}
pub struct Topology {
    grant: RegionGrant,
    windows: BTreeMap<u64, Window>,
    retired: BTreeSet<u64>,
    next_id: u64,
    revision: u64,
    focus: Option<u64>,
    focus_generation: u64,
    cursor: Point,
    cursor_shape: CursorShape,
    budget: MemoryBudget,
    _memory: MemoryReservation,
}
pub struct Projection {
    state: State,
    records: Vec<pb::Record>,
    _memory: MemoryReservation,
}
impl Projection {
    pub fn state(&self) -> &State {
        &self.state
    }
    pub fn records(&self) -> &[pb::Record] {
        &self.records
    }
}
impl Topology {
    pub fn new(grant: RegionGrant, budget: &MemoryBudget) -> Result<Self, ShellError> {
        let memory = budget.reserve(4 * 1024 * 1024)?;
        Ok(Self {
            grant,
            windows: BTreeMap::new(),
            retired: BTreeSet::new(),
            next_id: 16,
            revision: 1,
            focus: None,
            focus_generation: 1,
            cursor: Point {
                x: grant.logical().x,
                y: grant.logical().y,
            },
            cursor_shape: CursorShape::Default,
            budget: budget.clone(),
            _memory: memory,
        })
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn grant(&self) -> RegionGrant {
        self.grant
    }
    pub fn focused_surface(&self) -> Option<u64> {
        self.focus
    }
    fn advance(&mut self) -> Result<(), ShellError> {
        self.revision = self.revision.checked_add(1).ok_or(ShellError::Limit)?;
        Ok(())
    }
    fn change_routes(&mut self) -> Result<(), ShellError> {
        if self
            .windows
            .values()
            .any(|w| w.route_clock == u64::MAX || w.version == u64::MAX)
        {
            return Err(ShellError::Limit);
        }
        for window in self.windows.values_mut() {
            window.route_clock = window.route_clock.checked_add(1).ok_or(ShellError::Limit)?;
            window.version = window.version.checked_add(1).ok_or(ShellError::Limit)?;
        }
        Ok(())
    }
    fn preflight_change(&self, focus: bool) -> Result<(), ShellError> {
        if self.revision == u64::MAX
            || (focus && self.focus_generation == u64::MAX)
            || self
                .windows
                .values()
                .any(|w| w.version == u64::MAX || w.route_clock == u64::MAX)
        {
            return Err(ShellError::Limit);
        }
        Ok(())
    }
    pub fn open(&mut self, spec: WindowSpec) -> Result<u64, ShellError> {
        self.preflight_change(false)?;
        if self.windows.len() >= 16 {
            return Err(ShellError::Limit);
        }
        if spec
            .parent
            .is_some_and(|id| !self.windows.contains_key(&id))
            || spec
                .modal_for
                .is_some_and(|id| !self.windows.contains_key(&id))
        {
            return Err(ShellError::Geometry);
        }
        let id = self.next_id;
        let next = id.checked_add(6).ok_or(ShellError::Limit)?;
        let window = Window {
            id,
            version: 1,
            route_clock: 1,
            spec,
            mapped: true,
            text: String::new(),
            activity: 0,
        };
        self.windows.insert(id, window);
        // Reuse the core's finite/color/transform/ownership validation before
        // accepting any producer geometry into the authoritative topology.
        if let Err(error) = self.snapshot() {
            self.windows.remove(&id);
            return Err(error);
        }
        self.next_id = next;
        self.change_routes()?;
        self.advance()?;
        Ok(id)
    }
    fn descendant(&self, child: u64, ancestor: u64) -> bool {
        let mut current = Some(child);
        for _ in 0..16 {
            if current == Some(ancestor) {
                return true;
            }
            current = current
                .and_then(|id| self.windows.get(&id))
                .and_then(|w| w.spec.parent);
            if current.is_none() {
                return false;
            }
        }
        false
    }
    fn active_modal(&self) -> Option<u64> {
        self.windows
            .values()
            .filter(|w| w.mapped && w.spec.modal_for.is_some())
            .max_by_key(|w| (w.spec.stacking_order, w.id))
            .map(|w| w.id)
    }
    pub fn modal_allows(&self, surface: u64) -> bool {
        self.active_modal()
            .is_none_or(|modal| self.descendant(surface, modal))
    }
    pub fn focus(&mut self, surface: u64) -> Result<(), ShellError> {
        let window = self.windows.get(&surface).ok_or(ShellError::Geometry)?;
        if !window.mapped || window.spec.role == SurfaceRole::Tooltip || !self.modal_allows(surface)
        {
            return Err(ShellError::Input(
                rui_protocol_core::input::InputError::Unauthorized,
            ));
        }
        if self.focus != Some(surface) {
            self.preflight_change(true)?;
            self.focus = Some(surface);
            self.focus_generation = self
                .focus_generation
                .checked_add(1)
                .ok_or(ShellError::Limit)?;
            self.change_routes()?;
            self.advance()?;
        }
        Ok(())
    }
    pub fn close(&mut self, surface: u64) -> Result<(), ShellError> {
        self.preflight_change(true)?;
        if !self.windows.contains_key(&surface) {
            return Err(ShellError::Geometry);
        }
        let ids = self
            .windows
            .keys()
            .copied()
            .filter(|id| self.descendant(*id, surface))
            .collect::<Vec<_>>();
        if self.retired.len() + ids.len() * 6 > 4096 {
            return Err(ShellError::Limit);
        }
        let parent = self.windows[&surface].spec.parent;
        if self.focus.is_some_and(|focused| ids.contains(&focused)) {
            self.focus = None;
            self.focus_generation = self
                .focus_generation
                .checked_add(1)
                .ok_or(ShellError::Limit)?;
        }
        for id in ids {
            if let Some(window) = self.windows.remove(&id) {
                self.retired.extend(window.ids());
            }
        }
        self.change_routes()?;
        self.advance()?;
        if let Some(parent) =
            parent.filter(|id| self.windows.contains_key(id) && self.modal_allows(*id))
        {
            self.focus(parent)?;
        }
        Ok(())
    }
    pub fn set_mapped(&mut self, surface: u64, mapped: bool) -> Result<(), ShellError> {
        self.preflight_change(true)?;
        if !self.windows.contains_key(&surface) {
            return Err(ShellError::Geometry);
        }
        if self.windows[&surface].mapped != mapped {
            let descendants = self
                .windows
                .keys()
                .copied()
                .filter(|id| self.descendant(*id, surface))
                .collect::<Vec<_>>();
            if !mapped && self.focus.is_some_and(|id| descendants.contains(&id)) {
                self.focus = None;
                self.focus_generation += 1;
            }
            for id in descendants {
                self.windows.get_mut(&id).unwrap().mapped = mapped;
            }
            self.change_routes()?;
            self.advance()?;
        }
        Ok(())
    }
    /// Source-authorized move, content resize, scale/rotation and stacking
    /// update. Parent/modal links and incarnation remain unchanged.
    pub fn configure(
        &mut self,
        surface: u64,
        desktop_rect: Rect,
        content_size: Size,
        transform: Affine,
        stacking_order: i64,
    ) -> Result<(), ShellError> {
        self.preflight_change(false)?;
        let determinant = transform.xx * transform.yy - transform.xy * transform.yx;
        if !determinant.is_finite() || determinant == 0.0 {
            return Err(ShellError::Geometry);
        }
        let prior = self
            .windows
            .get(&surface)
            .ok_or(ShellError::Geometry)?
            .spec
            .clone();
        let window = self.windows.get_mut(&surface).unwrap();
        window.spec.desktop_rect = desktop_rect;
        window.spec.content_size = content_size;
        window.spec.transform = transform;
        window.spec.stacking_order = stacking_order;
        if let Err(error) = self.snapshot() {
            self.windows.get_mut(&surface).unwrap().spec = prior;
            return Err(error);
        }
        self.change_routes()?;
        self.advance()?;
        Ok(())
    }
    /// Authoritative input hit test: mapped, input-enabled surfaces sorted by
    /// (stacking_order,id); local clip/content bounds after inverse transform.
    /// Parent relationships never add offsets. Tooltip input regions are empty.
    pub fn hit_test(&self, desktop: Point) -> Option<u64> {
        self.windows
            .values()
            .filter(|w| w.mapped && w.spec.role != SurfaceRole::Tooltip)
            .filter(|w| {
                let m = w.spec.transform;
                let determinant = m.xx * m.yy - m.xy * m.yx;
                if determinant == 0.0 || !determinant.is_finite() {
                    return false;
                }
                let x = desktop.x - w.spec.desktop_rect.x - m.tx;
                let y = desktop.y - w.spec.desktop_rect.y - m.ty;
                let local = Point {
                    x: (m.yy * x - m.xy * y) / determinant,
                    y: (m.xx * y - m.yx * x) / determinant,
                };
                let b = w.bounds();
                local.x >= 0.0 && local.y >= 0.0 && local.x < b.width && local.y < b.height
            })
            .max_by_key(|w| (w.spec.stacking_order, w.id))
            .map(|w| w.id)
    }
    pub fn point_hits(&self, surface: u64, local: Point) -> Result<bool, ShellError> {
        Ok(self.hit_test(self.desktop_point(surface, local)?) == Some(surface))
    }
    pub fn route(&self, surface: u64, presented_revision: u64) -> Result<Route, ShellError> {
        let window = self.windows.get(&surface).ok_or(ShellError::Geometry)?;
        Ok(Route {
            surface,
            token: window.token(),
            presented_revision,
            bounds: window.bounds(),
            mapped: window.mapped,
            focused: self.focus == Some(surface),
            blocked_by_modal: !self.modal_allows(surface),
            authorized: window.spec.role != SurfaceRole::Tooltip,
            text_owner: TextOwner::HostIme,
        })
    }
    pub fn desktop_point(&self, surface: u64, point: Point) -> Result<Point, ShellError> {
        let window = self.windows.get(&surface).ok_or(ShellError::Geometry)?;
        let m = window.spec.transform;
        let result = Point {
            x: window.spec.desktop_rect.x + m.xx * point.x + m.xy * point.y + m.tx,
            y: window.spec.desktop_rect.y + m.yx * point.x + m.yy * point.y + m.ty,
        };
        self.grant.map(result)?;
        Ok(result)
    }
    pub fn native_point(&self, surface: u64, point: Point) -> Result<Point, ShellError> {
        self.grant.map(self.desktop_point(surface, point)?)
    }
    pub fn source_pointer(&mut self, native: Point) -> Result<(), ShellError> {
        let cursor = self.grant.unmap(native)?;
        self.advance()?;
        self.cursor = cursor;
        Ok(())
    }
    pub fn set_cursor(&mut self, shape: CursorShape) -> Result<(), ShellError> {
        self.advance()?;
        self.cursor_shape = shape;
        Ok(())
    }
    pub fn source_activity(&mut self, surface: u64, text: &str) -> Result<(), ShellError> {
        self.preflight_change(false)?;
        let window = self.windows.get_mut(&surface).ok_or(ShellError::Geometry)?;
        if window.activity == u64::MAX {
            return Err(ShellError::Limit);
        }
        if window.text.len() + text.len() > 4096 {
            return Err(ShellError::Limit);
        }
        window.text.push_str(text);
        window.activity = window.activity.checked_add(1).ok_or(ShellError::Limit)?;
        window.version = window.version.checked_add(1).ok_or(ShellError::Limit)?;
        self.advance()?;
        Ok(())
    }
    /// The vector laboratory applies one click effect on a real button press.
    /// Releases neutralize held input and never activate the application again.
    /// Both native EIS and source-local GDK callbacks use this source operation.
    pub fn source_button(&mut self, surface: u64, pressed: bool) -> Result<(), ShellError> {
        if !self.windows.contains_key(&surface) {
            return Err(ShellError::Geometry);
        }
        if pressed {
            self.source_activity(surface, "")?;
        }
        Ok(())
    }
    pub fn text(&self, surface: u64) -> Option<&str> {
        self.windows.get(&surface).map(|w| w.text.as_str())
    }
    pub fn snapshot(&self) -> Result<Projection, ShellError> {
        // Both the producer records and converted state fit this bounded
        // staging allowance; escaped outputs retain their actual charge.
        let mut memory = self.budget.reserve(2 * 1024 * 1024)?;
        let records = self.build_records();
        let mut state =
            State::from_source_records(self.revision, records.clone(), &Limits::default())?;
        state.retired = self.retired.clone();
        // Known producer vocabulary: 4096 per record bounds all fixed proto
        // fields, bounded vectors and metadata; public text charged separately.
        let bytes = state.estimated_bytes()
            + records.len() * 4096
            + self
                .windows
                .values()
                .map(|w| w.text.capacity())
                .sum::<usize>()
            + 2048;
        memory.resize(bytes)?;
        Ok(Projection {
            state,
            records,
            _memory: memory,
        })
    }
    fn build_records(&self) -> Vec<pb::Record> {
        let mut records = Vec::with_capacity(1 + self.windows.len() * 6);
        let focused = self.focus.and_then(|id| self.windows.get(&id));
        records.push(record(pb::record::Record::Session(pb::SessionState {
            id: 1,
            version: self.revision,
            canvas: Some(rect(self.grant.logical())),
            cursor: Some(pb::Cursor {
                position: Some(point(self.cursor)),
                hotspot: Some(pb::Point { x: 0.0, y: 0.0 }),
                appearance: Some(pb::cursor::Appearance::Shape(match self.cursor_shape {
                    CursorShape::Default => 1,
                    CursorShape::Text => 2,
                    CursorShape::Pointer => 3,
                    CursorShape::Hidden => 4,
                })),
            }),
            focus: focused.map(|w| reference(3, w.semantic())),
            focus_surface: focused.map(|w| reference(1, w.id)),
            focus_generation: self.focus_generation,
        })));
        for window in self.windows.values() {
            let bounds = window.bounds();
            records.push(record(pb::record::Record::Surface(pb::Surface {
                id: window.id,
                version: window.version,
                role: match window.spec.role {
                    SurfaceRole::Toplevel => 1,
                    SurfaceRole::Popup => 2,
                    SurfaceRole::Tooltip => 3,
                    SurfaceRole::Shell => 4,
                },
                parent_surface: window.spec.parent.map(|id| reference(1, id)),
                desktop_rect: Some(rect(window.spec.desktop_rect)),
                content_size: Some(size(window.spec.content_size)),
                transform: Some(affine(window.spec.transform)),
                clip: Some(rect(bounds)),
                opacity: 1.0,
                stacking_order: window.spec.stacking_order,
                mapped: window.mapped,
                focused: self.focus == Some(window.id),
                modal_for: window.spec.modal_for.map(|id| reference(1, id)),
                scene_root: Some(reference(2, window.root())),
                semantic_root: Some(reference(3, window.semantic())),
                input_region: if window.spec.role == SurfaceRole::Tooltip {
                    vec![]
                } else {
                    vec![rect(bounds)]
                },
                routing_token: window.token().to_vec(),
                visual_coverage: 1,
                semantic_coverage: 1,
                capture_consistency: 1,
                source_capture_tokens: vec![window.version.to_be_bytes().to_vec()],
                unsupported_features: vec![],
            })));
            records.push(record(pb::record::Record::Scene(pb::SceneNode {
                id: window.root(),
                version: window.version,
                ordered_children: vec![
                    reference(2, window.background()),
                    reference(2, window.indicator()),
                ],
                transform: Some(identity()),
                opacity: 1.0,
                primitive: Some(pb::scene_node::Primitive::Group(pb::Group {})),
                ..Default::default()
            })));
            records.push(record(pb::record::Record::Scene(solid(
                window.background(),
                window.version,
                bounds,
                window.spec.color,
            ))));
            let activity = (window.activity % 16) as f64 / 16.0;
            records.push(record(pb::record::Record::Scene(solid(
                window.indicator(),
                window.version,
                Rect {
                    x: bounds.width * 0.1,
                    y: bounds.height * 0.1,
                    width: bounds.width * 0.8,
                    height: bounds.height * 0.2,
                },
                Color {
                    red: activity,
                    green: 0.25,
                    blue: 1.0 - activity,
                    alpha: 1.0,
                },
            ))));
            records.push(record(pb::record::Record::Semantic(pb::SemanticNode {
                browser_input_type: None,
                id: window.semantic(),
                version: window.version,
                role: 9,
                label: Some("Isolated vector input laboratory".into()),
                typed_value: Some(pb::TypedValue {
                    value: Some(pb::typed_value::Value::Text(window.text.clone())),
                }),
                visual_bindings: vec![reference(5, window.binding())],
                sensitivity: 1,
                completeness: 1,
                ..Default::default()
            })));
            records.push(record(pb::record::Record::Binding(pb::Binding {
                id: window.binding(),
                version: window.version,
                surface: Some(reference(1, window.id)),
                semantic_nodes: vec![reference(3, window.semantic())],
                scene_nodes: vec![reference(2, window.indicator())],
                interaction_region: Some(rect(bounds)),
            })));
        }
        records
    }
}
fn record(record: pb::record::Record) -> pb::Record {
    pb::Record {
        record: Some(record),
    }
}
fn reference(kind: i32, id: u64) -> pb::ObjectRef {
    pb::ObjectRef { kind, id }
}
fn point(p: Point) -> pb::Point {
    pb::Point { x: p.x, y: p.y }
}
fn rect(r: Rect) -> pb::Rect {
    pb::Rect {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
    }
}
fn size(s: Size) -> pb::Size {
    pb::Size {
        width: s.width,
        height: s.height,
    }
}
fn affine(m: Affine) -> pb::Affine {
    pb::Affine {
        xx: m.xx,
        yx: m.yx,
        xy: m.xy,
        yy: m.yy,
        tx: m.tx,
        ty: m.ty,
    }
}
fn identity() -> pb::Affine {
    pb::Affine {
        xx: 1.0,
        yy: 1.0,
        ..Default::default()
    }
}
fn solid(id: u64, version: u64, r: Rect, c: Color) -> pb::SceneNode {
    pb::SceneNode {
        id,
        version,
        transform: Some(identity()),
        opacity: 1.0,
        primitive: Some(pb::scene_node::Primitive::SolidRect(pb::SolidRect {
            rect: Some(rect(r)),
            color: Some(pb::Color {
                red: c.red,
                green: c.green,
                blue: c.blue,
                alpha: c.alpha,
            }),
        })),
        ..Default::default()
    }
}
