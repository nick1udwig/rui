use crate::path::{self, PathMask, StrokeStyle};
use rui_protocol_core::model::{
    Affine, Color, CornerRadii, CursorAppearance, CursorShape, FillRule, GlyphFont, GradientStop,
    ObjectKind, ObjectRef, Point, Primitive, Record, Rect, State, VisualCoverage,
};
use rui_protocol_core::resources::{
    MemoryBudget, MemoryError, MemoryReservation, ResourceError, ResourceKind, ResourceOffer,
};
use std::collections::BTreeSet;
use std::io::{self, Write};
use thiserror::Error;

pub const PROFILE: &str = "renderer.cpu-samples2x2.cairo1184.v1";
pub const CAIRO_VERSION: &str = "1.18.4";

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("pinned Cairo version required: expected 1.18.4, got {0}")]
    BackendVersion(String),
    #[error("render limit exceeded: {0}")]
    Limit(&'static str),
    #[error("render allocation failed")]
    Allocation,
    #[error("invalid geometry, color, graph or transform")]
    Invalid,
    #[error("missing render resource: {0:?}")]
    MissingResource([u8; 32]),
    #[error("resource descriptor does not match this primitive")]
    ResourceDescriptor,
    #[error("exact resource digest mismatch")]
    ResourceIntegrity,
    #[error("font fingerprint is unavailable; this profile requires glyph.outline-single.v1")]
    MissingFont,
    #[error("single-outline profile requires every glyph ID to be zero")]
    UnsupportedGlyph,
    #[error("surface is partial or unsupported: {0}")]
    IncompleteSurface(u64),
    #[error("cursor shape has no defined paint in this renderer profile")]
    UnsupportedCursor,
    #[error(transparent)]
    Resource(#[from] ResourceError),
    #[error(transparent)]
    Memory(#[from] MemoryError),
    #[error(transparent)]
    Cairo(#[from] cairo::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Clone, Debug)]
pub struct RenderLimits {
    pub max_pixels: usize,
    pub max_dimension: u32,
    pub max_nodes: usize,
    pub max_depth: usize,
    pub max_work: u64,
    pub max_path_commands: usize,
    pub max_flat_segments: usize,
    pub max_curve_depth: usize,
    pub curve_tolerance_pixels: f64,
    pub max_path_coordinate: f64,
    pub max_resource_bytes: usize,
    pub max_dash_intervals: usize,
    pub max_stroke_width: f64,
    pub max_miter_limit: f64,
    pub max_dash_lengths: usize,
    pub max_gradient_stops: usize,
    pub max_glyphs: usize,
    pub max_transform_scale: f64,
}
impl Default for RenderLimits {
    fn default() -> Self {
        Self {
            max_pixels: 1_048_576,
            max_dimension: 4096,
            max_nodes: 10_000,
            max_depth: 128,
            max_work: 200_000_000,
            max_path_commands: 4096,
            max_flat_segments: 16_384,
            max_curve_depth: 20,
            curve_tolerance_pixels: 1.0 / 256.0,
            max_path_coordinate: 1_000_000.0,
            max_resource_bytes: 16 << 20,
            max_dash_intervals: 4096,
            max_stroke_width: 4096.0,
            max_miter_limit: 32.0,
            max_dash_lengths: 1024,
            max_gradient_stops: 4096,
            max_glyphs: 4096,
            max_transform_scale: 1_000_000.0,
        }
    }
}

/// Provider implementations must return only resources authorized for the
/// current view. Renderer verifies descriptor, exact hash and vector structure.
pub struct ResourceView<'a> {
    pub offer: &'a ResourceOffer,
    pub bytes: &'a [u8],
}
pub trait ResourceProvider {
    fn get(&self, digest: &[u8; 32]) -> Option<ResourceView<'_>>;
}
impl ResourceProvider for () {
    fn get(&self, _: &[u8; 32]) -> Option<ResourceView<'_>> {
        None
    }
}
impl ResourceProvider for rui_protocol_core::receiver::ViewReceiver {
    fn get(&self, digest: &[u8; 32]) -> Option<ResourceView<'_>> {
        Some(ResourceView {
            offer: self.resource_offer(*digest).ok()?,
            bytes: self.read_resource(*digest).ok()?,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub origin: Point,
    pub scale: f64,
}
impl Viewport {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            origin: Point { x: 0.0, y: 0.0 },
            scale: 1.0,
        }
    }
}

/// Premultiplied linear-sRGB, before the output display transfer function.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LinearPixel {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub alpha: f64,
}
impl LinearPixel {
    fn from_color(c: Color) -> Self {
        Self {
            red: c.red,
            green: c.green,
            blue: c.blue,
            alpha: c.alpha,
        }
    }
    fn over(self, destination: Self) -> Self {
        let inverse = 1.0 - self.alpha;
        Self {
            red: self.red + destination.red * inverse,
            green: self.green + destination.green * inverse,
            blue: self.blue + destination.blue * inverse,
            alpha: self.alpha + destination.alpha * inverse,
        }
    }
    fn multiply(self, value: f64) -> Self {
        Self {
            red: self.red * value,
            green: self.green * value,
            blue: self.blue * value,
            alpha: self.alpha * value,
        }
    }
    fn add(self, other: Self) -> Self {
        Self {
            red: self.red + other.red,
            green: self.green + other.green,
            blue: self.blue + other.blue,
            alpha: self.alpha + other.alpha,
        }
    }
    pub fn rgba8(self) -> [u8; 4] {
        if self.alpha <= 0.0 {
            return [0, 0, 0, 0];
        }
        [
            display(self.red / self.alpha),
            display(self.green / self.alpha),
            display(self.blue / self.alpha),
            quantize(self.alpha),
        ]
    }
}
fn quantize(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8
}
fn display(value: f64) -> u8 {
    let value = value.clamp(0.0, 1.0);
    quantize(if value <= 0.0031308 {
        12.92 * value
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    })
}

pub struct Frame {
    pub revision: u64,
    pub viewport: Viewport,
    pixels: Vec<LinearPixel>,
    pub work: u64,
    budget: MemoryBudget,
    _memory: MemoryReservation,
}
impl Frame {
    pub fn pixels(&self) -> &[LinearPixel] {
        &self.pixels
    }
    pub fn pixel(&self, x: u32, y: u32) -> Option<LinearPixel> {
        if x >= self.viewport.width || y >= self.viewport.height {
            return None;
        }
        self.pixels
            .get(y as usize * self.viewport.width as usize + x as usize)
            .copied()
    }
    /// Portable alpha map: unpremultiplied display-sRGB RGB and linear alpha.
    pub fn write_pam(&self, writer: &mut impl Write) -> Result<(), RenderError> {
        let _memory = self
            .budget
            .reserve(self.viewport.width as usize * 4 + 256)?;
        write!(
            writer,
            "P7\nWIDTH {}\nHEIGHT {}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n",
            self.viewport.width, self.viewport.height
        )?;
        let mut row = Vec::new();
        row.try_reserve_exact(self.viewport.width as usize * 4)
            .map_err(|_| RenderError::Allocation)?;
        for pixels in self.pixels.chunks_exact(self.viewport.width as usize) {
            row.clear();
            for pixel in pixels {
                row.extend_from_slice(&pixel.rgba8());
            }
            writer.write_all(&row)?;
        }
        Ok(())
    }
    /// Portable pixmap composed over a supplied opaque linear background.
    pub fn write_ppm(&self, writer: &mut impl Write, background: Color) -> Result<(), RenderError> {
        valid_color(background)?;
        if background.alpha != 1.0 {
            return Err(RenderError::Invalid);
        }
        let _memory = self
            .budget
            .reserve(self.viewport.width as usize * 3 + 256)?;
        write!(
            writer,
            "P6\n{} {}\n255\n",
            self.viewport.width, self.viewport.height
        )?;
        let mut row = Vec::new();
        row.try_reserve_exact(self.viewport.width as usize * 3)
            .map_err(|_| RenderError::Allocation)?;
        for pixels in self.pixels.chunks_exact(self.viewport.width as usize) {
            row.clear();
            for pixel in pixels {
                row.extend_from_slice(
                    &pixel.over(LinearPixel::from_color(background)).rgba8()[..3],
                );
            }
            writer.write_all(&row)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Matrix {
    xx: f64,
    yx: f64,
    xy: f64,
    yy: f64,
    tx: f64,
    ty: f64,
}
impl Matrix {
    fn identity() -> Self {
        Self {
            xx: 1.0,
            yx: 0.0,
            xy: 0.0,
            yy: 1.0,
            tx: 0.0,
            ty: 0.0,
        }
    }
    fn from_affine(m: Affine) -> Self {
        Self {
            xx: m.xx,
            yx: m.yx,
            xy: m.xy,
            yy: m.yy,
            tx: m.tx,
            ty: m.ty,
        }
    }
    fn translated(x: f64, y: f64) -> Self {
        Self {
            tx: x,
            ty: y,
            ..Self::identity()
        }
    }
    fn compose(self, r: Self) -> Self {
        Self {
            xx: self.xx * r.xx + self.xy * r.yx,
            yx: self.yx * r.xx + self.yy * r.yx,
            xy: self.xx * r.xy + self.xy * r.yy,
            yy: self.yx * r.xy + self.yy * r.yy,
            tx: self.xx * r.tx + self.xy * r.ty + self.tx,
            ty: self.yx * r.tx + self.yy * r.ty + self.ty,
        }
    }
    fn point(self, p: Point) -> Point {
        Point {
            x: self.xx * p.x + self.xy * p.y + self.tx,
            y: self.yx * p.x + self.yy * p.y + self.ty,
        }
    }
    fn scale_bound(self) -> f64 {
        self.xx.abs() + self.yx.abs() + self.xy.abs() + self.yy.abs()
    }
    fn inverse(self, limits: &RenderLimits) -> Result<Option<Self>, RenderError> {
        if [self.xx, self.xy, self.yx, self.yy, self.tx, self.ty]
            .iter()
            .any(|n| !n.is_finite())
            || self.scale_bound() > limits.max_transform_scale
        {
            return Err(RenderError::Invalid);
        }
        let determinant = self.xx * self.yy - self.xy * self.yx;
        if determinant == 0.0 {
            return Ok(None);
        }
        let inverse = Self {
            xx: self.yy / determinant,
            yx: -self.yx / determinant,
            xy: -self.xy / determinant,
            yy: self.xx / determinant,
            tx: (self.xy * self.ty - self.yy * self.tx) / determinant,
            ty: (self.yx * self.tx - self.xx * self.ty) / determinant,
        };
        if [
            inverse.xx, inverse.xy, inverse.yx, inverse.yy, inverse.tx, inverse.ty,
        ]
        .iter()
        .any(|n| !n.is_finite())
        {
            return Err(RenderError::Invalid);
        }
        Ok(Some(inverse))
    }
}

enum Paint<'a> {
    Group(Vec<usize>),
    Rect(Rect, Color),
    Rounded(Rect, CornerRadii, Color),
    Border(Rect, CornerRadii, Rect, CornerRadii, Color),
    Linear(Rect, Point, Point, &'a [GradientStop]),
    Radial(Rect, Point, f64, f64, &'a [GradientStop]),
    Path(PathMask, Color),
    Glyph(PathMask, f64, &'a [Point], Color),
}
struct Node<'a> {
    bounds: Option<Bounds>,
    inverse: Option<Matrix>,
    clip: Option<Rect>,
    opacity: f64,
    paint: Paint<'a>,
}
struct SurfacePlan {
    bounds: Option<Bounds>,
    root: usize,
    inverse: Option<Matrix>,
    content: Rect,
    clip: Option<Rect>,
    opacity: f64,
}
/// Conservative desktop-space bounds. Inclusive culling never removes a path
/// boundary sample; exact primitive membership still decides paint coverage.
#[derive(Clone, Copy)]
struct Bounds {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
}
impl Bounds {
    fn rect(r: Rect) -> Option<Self> {
        if r.width == 0.0 || r.height == 0.0 {
            None
        } else {
            Some(Self {
                x1: r.x,
                y1: r.y,
                x2: r.x + r.width,
                y2: r.y + r.height,
            })
        }
    }
    fn path(p: &PathMask) -> Option<Self> {
        let (x1, y1, x2, y2) = p.bounds();
        if x1 >= x2 || y1 >= y2 {
            None
        } else {
            Some(Self { x1, y1, x2, y2 })
        }
    }
    fn contains(self, p: Point) -> bool {
        p.x >= self.x1 && p.x <= self.x2 && p.y >= self.y1 && p.y <= self.y2
    }
    fn union(self, b: Self) -> Self {
        Self {
            x1: self.x1.min(b.x1),
            y1: self.y1.min(b.y1),
            x2: self.x2.max(b.x2),
            y2: self.y2.max(b.y2),
        }
    }
    fn intersect(self, b: Self) -> Option<Self> {
        let r = Self {
            x1: self.x1.max(b.x1),
            y1: self.y1.max(b.y1),
            x2: self.x2.min(b.x2),
            y2: self.y2.min(b.y2),
        };
        if r.x1 > r.x2 || r.y1 > r.y2 {
            None
        } else {
            Some(r)
        }
    }
    fn transformed(self, m: Matrix) -> Result<Self, RenderError> {
        let p = [
            m.point(Point {
                x: self.x1,
                y: self.y1,
            }),
            m.point(Point {
                x: self.x1,
                y: self.y2,
            }),
            m.point(Point {
                x: self.x2,
                y: self.y1,
            }),
            m.point(Point {
                x: self.x2,
                y: self.y2,
            }),
        ];
        for point in p {
            valid_point(point)?;
        }
        let condition = m.scale_bound().powi(2) / (m.xx * m.yy - m.xy * m.yx).abs();
        // Culling is optional. Disable it for unstable inverse transforms rather
        // than trust a tight forward bound under severe cancellation.
        if !condition.is_finite() || condition > 1e12 {
            return Ok(Self {
                x1: -f64::MAX,
                y1: -f64::MAX,
                x2: f64::MAX,
                y2: f64::MAX,
            });
        }
        let magnitude = m.scale_bound()
            * (self.x1.abs().max(self.x2.abs()) + self.y1.abs().max(self.y2.abs()))
            + m.tx.abs()
            + m.ty.abs()
            + 1.0;
        let padding = 64.0 * f64::EPSILON * condition * magnitude;
        if !padding.is_finite() {
            return Ok(Self {
                x1: -f64::MAX,
                y1: -f64::MAX,
                x2: f64::MAX,
                y2: f64::MAX,
            });
        }
        Ok(Self {
            x1: (p.iter().map(|p| p.x).fold(f64::INFINITY, f64::min) - padding).next_down(),
            y1: (p.iter().map(|p| p.y).fold(f64::INFINITY, f64::min) - padding).next_down(),
            x2: (p.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max) + padding).next_up(),
            y2: (p.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max) + padding).next_up(),
        })
    }
}
struct Plan<'a> {
    nodes: Vec<Node<'a>>,
    surfaces: Vec<SurfacePlan>,
    canvas: Option<Rect>,
    work: u64,
    _memory: MemoryReservation,
}

pub struct Renderer {
    pub limits: RenderLimits,
    budget: MemoryBudget,
}
impl Renderer {
    pub fn new(limits: RenderLimits, budget: MemoryBudget) -> Result<Self, RenderError> {
        if cairo::version_string() != CAIRO_VERSION {
            return Err(RenderError::BackendVersion(cairo::version_string().into()));
        }
        if !limits.curve_tolerance_pixels.is_finite()
            || limits.curve_tolerance_pixels <= 0.0
            || limits.max_flat_segments == 0
            || limits.max_curve_depth > 32
            || !limits.max_transform_scale.is_finite()
            || limits.max_transform_scale <= 0.0
            || !limits.max_path_coordinate.is_finite()
            || limits.max_path_coordinate <= 0.0
            || !limits.max_stroke_width.is_finite()
            || limits.max_stroke_width <= 0.0
            || !limits.max_miter_limit.is_finite()
            || !(1.0..=32.0).contains(&limits.max_miter_limit)
            || limits.max_gradient_stops > 4096
            || limits.max_dash_lengths > 1024
        {
            return Err(RenderError::Invalid);
        }
        Ok(Self { limits, budget })
    }
    /// Renders one retained root without surface composition, for reference tests.
    pub fn render_scene(
        &self,
        state: &State,
        root: ObjectRef,
        viewport: Viewport,
        resources: &impl ResourceProvider,
    ) -> Result<Frame, RenderError> {
        self.validate_viewport(viewport)?;
        let mut plan = self.plan(state)?;
        let mut visited = BTreeSet::new();
        let index = self.prepare(
            state,
            root,
            Matrix::identity(),
            viewport.scale,
            resources,
            &mut plan,
            &mut visited,
            0,
        )?;
        plan.surfaces.push(SurfacePlan {
            bounds: plan.nodes[index].bounds,
            root: index,
            inverse: Some(Matrix::identity()),
            content: Rect {
                x: f64::NEG_INFINITY,
                y: f64::NEG_INFINITY,
                width: f64::INFINITY,
                height: f64::INFINITY,
            },
            clip: None,
            opacity: 1.0,
        });
        self.raster(state.revision, viewport, &plan)
    }
    pub fn render_view(
        &self,
        state: &State,
        viewport: Viewport,
        resources: &impl ResourceProvider,
    ) -> Result<Frame, RenderError> {
        self.validate_viewport(viewport)?;
        let mut plan = self.plan(state)?;
        let mut visited = BTreeSet::new();
        let mut surfaces = state
            .records
            .values()
            .filter_map(|r| match r {
                Record::Surface(s) if s.mapped => Some(s),
                _ => None,
            })
            .collect::<Vec<_>>();
        surfaces.sort_by_key(|s| (s.stacking_order, s.id));
        for surface in surfaces {
            if surface.visual_coverage != VisualCoverage::Complete {
                return Err(RenderError::IncompleteSurface(surface.id));
            }
            valid_rect(surface.desktop_rect)?;
            valid_opacity(surface.opacity)?;
            let matrix = Matrix::translated(surface.desktop_rect.x, surface.desktop_rect.y)
                .compose(Matrix::from_affine(surface.transform));
            let inverse = matrix.inverse(&self.limits)?;
            let content = Rect {
                x: 0.0,
                y: 0.0,
                width: surface.content_size.width,
                height: surface.content_size.height,
            };
            valid_rect(content)?;
            if let Some(clip) = surface.clip {
                valid_rect(clip)?;
            }
            let root = self.prepare(
                state,
                surface.scene_root.ok_or(RenderError::Invalid)?,
                matrix,
                viewport.scale,
                resources,
                &mut plan,
                &mut visited,
                0,
            )?;
            plan.surfaces.push(SurfacePlan {
                bounds: plan.nodes[root].bounds,
                root,
                inverse,
                content,
                clip: surface.clip,
                opacity: surface.opacity,
            });
        }
        for record in state.records.values() {
            if let Record::Session(session) = record {
                valid_rect(session.canvas)?;
                plan.canvas = Some(session.canvas);
                if let Some(cursor) = &session.cursor {
                    if !matches!(
                        cursor.appearance,
                        CursorAppearance::Shape(CursorShape::Hidden)
                    ) {
                        return Err(RenderError::UnsupportedCursor);
                    }
                }
            }
        }
        self.raster(state.revision, viewport, &plan)
    }
    fn plan<'a>(&self, state: &'a State) -> Result<Plan<'a>, RenderError> {
        let count = state
            .records
            .values()
            .filter(|r| matches!(r, Record::Scene(_)))
            .count();
        if count > self.limits.max_nodes {
            return Err(RenderError::Limit("scene nodes"));
        }
        let count = count.max(1);
        let surface_count = state
            .records
            .values()
            .filter(|r| matches!(r,Record::Surface(s) if s.mapped))
            .count();
        let children = state
            .records
            .values()
            .filter_map(|r| match r {
                Record::Scene(node) => Some(node.children.len()),
                _ => None,
            })
            .try_fold(0usize, usize::checked_add)
            .ok_or(RenderError::Limit("scene edges"))?;
        if children > self.limits.max_nodes || surface_count > self.limits.max_nodes {
            return Err(RenderError::Limit("scene edges/surfaces"));
        }
        // Node slots, child indexes, visited IDs, surfaces and collection slack.
        let memory = self.budget.reserve(
            count
                .saturating_mul(std::mem::size_of::<Node<'_>>() + 256)
                .saturating_add(
                    surface_count
                        .max(1)
                        .saturating_mul(std::mem::size_of::<SurfacePlan>() + 32),
                )
                .saturating_add(4096),
        )?;
        let mut nodes = Vec::new();
        nodes
            .try_reserve_exact(count)
            .map_err(|_| RenderError::Allocation)?;
        let mut surfaces = Vec::new();
        surfaces
            .try_reserve_exact(surface_count.max(1))
            .map_err(|_| RenderError::Allocation)?;
        Ok(Plan {
            nodes,
            surfaces,
            canvas: None,
            work: 0,
            _memory: memory,
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn prepare<'a>(
        &self,
        state: &'a State,
        reference: ObjectRef,
        parent: Matrix,
        viewport_scale: f64,
        resources: &impl ResourceProvider,
        plan: &mut Plan<'a>,
        visited: &mut BTreeSet<u64>,
        depth: usize,
    ) -> Result<usize, RenderError> {
        if depth >= self.limits.max_depth {
            return Err(RenderError::Limit("scene depth"));
        }
        charge(&mut plan.work, 1, &self.limits)?;
        if reference.kind != ObjectKind::Scene || !visited.insert(reference.id) {
            return Err(RenderError::Invalid);
        }
        let Some(Record::Scene(node)) = state.records.get(&reference.id) else {
            return Err(RenderError::Invalid);
        };
        if node.id != reference.id {
            return Err(RenderError::Invalid);
        }
        let matrix = parent.compose(Matrix::from_affine(node.transform));
        let inverse = matrix.inverse(&self.limits)?;
        if let Some(clip) = node.clip {
            valid_rect(clip)?;
        }
        valid_opacity(node.opacity)?;
        let scale = matrix.scale_bound() * viewport_scale;
        let paint = match &node.primitive {
            Primitive::Group => {
                let mut children = Vec::new();
                children
                    .try_reserve_exact(node.children.len())
                    .map_err(|_| RenderError::Allocation)?;
                for child in &node.children {
                    children.push(self.prepare(
                        state,
                        *child,
                        matrix,
                        viewport_scale,
                        resources,
                        plan,
                        visited,
                        depth + 1,
                    )?);
                }
                Paint::Group(children)
            }
            Primitive::SolidRect { rect, color } => {
                valid_rect(*rect)?;
                valid_color(*color)?;
                Paint::Rect(*rect, *color)
            }
            Primitive::RoundedRect { rect, radii, color } => {
                valid_rect(*rect)?;
                valid_radii(*rect, *radii)?;
                valid_color(*color)?;
                Paint::Rounded(*rect, *radii, *color)
            }
            Primitive::Border {
                rect,
                radii,
                width,
                color,
            } => {
                valid_rect(*rect)?;
                valid_radii(*rect, *radii)?;
                if !width.is_finite() || *width < 0.0 || *width > rect.width.min(rect.height) / 2.0
                {
                    return Err(RenderError::Invalid);
                }
                valid_color(*color)?;
                let (inner, inner_radii) = inset(*rect, *radii, *width);
                valid_radii(inner, inner_radii)?;
                Paint::Border(*rect, *radii, inner, inner_radii, *color)
            }
            Primitive::LinearGradient {
                rect,
                start,
                end,
                stops,
                ..
            } => {
                valid_rect(*rect)?;
                valid_point(*start)?;
                valid_point(*end)?;
                let length = (end.x - start.x).hypot(end.y - start.y);
                if !length.is_finite() || length == 0.0 {
                    return Err(RenderError::Invalid);
                }
                charge(&mut plan.work, stops.len() as u64, &self.limits)?;
                if stops.len() > self.limits.max_gradient_stops {
                    return Err(RenderError::Limit("gradient stops"));
                }
                valid_stops(stops)?;
                Paint::Linear(*rect, *start, *end, stops)
            }
            Primitive::RadialGradient {
                rect,
                center,
                inner_radius,
                outer_radius,
                stops,
                ..
            } => {
                valid_rect(*rect)?;
                valid_point(*center)?;
                if !inner_radius.is_finite()
                    || !outer_radius.is_finite()
                    || *inner_radius < 0.0
                    || *outer_radius <= *inner_radius
                    || !(center.x + outer_radius).is_finite()
                    || !(center.x - outer_radius).is_finite()
                    || !(center.y + outer_radius).is_finite()
                    || !(center.y - outer_radius).is_finite()
                {
                    return Err(RenderError::Invalid);
                }
                charge(&mut plan.work, stops.len() as u64, &self.limits)?;
                if stops.len() > self.limits.max_gradient_stops {
                    return Err(RenderError::Limit("gradient stops"));
                }
                valid_stops(stops)?;
                Paint::Radial(*rect, *center, *inner_radius, *outer_radius, stops)
            }
            Primitive::FillPath {
                resource,
                color,
                fill_rule,
            } => {
                valid_color(*color)?;
                Paint::Path(
                    path::compile(
                        *resource,
                        ResourceKind::VectorPath,
                        resources,
                        *fill_rule,
                        None,
                        scale,
                        &self.limits,
                        &self.budget,
                        &mut plan.work,
                    )?,
                    *color,
                )
            }
            Primitive::StrokePath {
                resource,
                color,
                width,
                cap,
                join,
                miter_limit,
                dash_lengths,
                dash_offset,
            } => {
                valid_color(*color)?;
                Paint::Path(
                    path::compile(
                        *resource,
                        ResourceKind::VectorPath,
                        resources,
                        FillRule::Nonzero,
                        Some(StrokeStyle {
                            width: *width,
                            cap: *cap,
                            join: *join,
                            miter_limit: *miter_limit,
                            dashes: dash_lengths,
                            offset: *dash_offset,
                        }),
                        scale,
                        &self.limits,
                        &self.budget,
                        &mut plan.work,
                    )?,
                    *color,
                )
            }
            Primitive::GlyphRun {
                font,
                size,
                glyph_ids,
                positions,
                color,
                ..
            } => {
                valid_color(*color)?;
                let resource = match font {
                    GlyphFont::Fingerprint(_) => return Err(RenderError::MissingFont),
                    GlyphFont::Outline(resource) => *resource,
                };
                if glyph_ids.iter().any(|id| *id != 0) || glyph_ids.len() != positions.len() {
                    return Err(RenderError::UnsupportedGlyph);
                }
                if !size.is_finite() || *size <= 0.0 {
                    return Err(RenderError::Invalid);
                }
                if positions.len() > self.limits.max_glyphs {
                    return Err(RenderError::Limit("glyphs"));
                }
                charge(&mut plan.work, positions.len() as u64, &self.limits)?;
                for position in positions {
                    valid_point(*position)?;
                }
                Paint::Glyph(
                    path::compile(
                        resource,
                        ResourceKind::GlyphOutline,
                        resources,
                        FillRule::Nonzero,
                        None,
                        scale * size,
                        &self.limits,
                        &self.budget,
                        &mut plan.work,
                    )?,
                    *size,
                    positions,
                    *color,
                )
            }
        };
        if !matches!(node.primitive, Primitive::Group) && !node.children.is_empty() {
            return Err(RenderError::Invalid);
        }
        let index = plan.nodes.len();
        let mut bounds = match &paint {
            Paint::Group(children) => children
                .iter()
                .filter_map(|i| plan.nodes[*i].bounds)
                .reduce(Bounds::union),
            Paint::Rect(rect, _)
            | Paint::Rounded(rect, ..)
            | Paint::Border(rect, ..)
            | Paint::Linear(rect, ..)
            | Paint::Radial(rect, ..) => Bounds::rect(*rect)
                .map(|b| b.transformed(matrix))
                .transpose()?,
            Paint::Path(path, _) => Bounds::path(path)
                .map(|b| b.transformed(matrix))
                .transpose()?,
            Paint::Glyph(path, size, positions, _) => {
                let mut result: Option<Bounds> = None;
                if let Some(b) = Bounds::path(path) {
                    for position in *positions {
                        let glyph_matrix = matrix.compose(Matrix {
                            xx: *size,
                            yy: *size,
                            tx: position.x,
                            ty: position.y,
                            xy: 0.0,
                            yx: 0.0,
                        });
                        let b = b.transformed(glyph_matrix)?;
                        result = Some(result.map_or(b, |r| r.union(b)));
                    }
                }
                result
            }
        };
        if let Some(clip) = node.clip {
            bounds = match (bounds, Bounds::rect(clip)) {
                (Some(b), Some(c)) => b.intersect(c.transformed(matrix)?),
                _ => None,
            };
        }
        if inverse.is_none() || node.opacity == 0.0 {
            bounds = None;
        }
        plan.nodes.push(Node {
            bounds,
            inverse,
            clip: node.clip,
            opacity: node.opacity,
            paint,
        });
        Ok(index)
    }
    fn raster(
        &self,
        revision: u64,
        viewport: Viewport,
        plan: &Plan<'_>,
    ) -> Result<Frame, RenderError> {
        let count = self.validate_viewport(viewport)?;
        let memory = self.budget.reserve(
            count
                .checked_mul(std::mem::size_of::<LinearPixel>())
                .and_then(|n| n.checked_add(256))
                .ok_or(RenderError::Limit("frame bytes"))?,
        )?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(count)
            .map_err(|_| RenderError::Allocation)?;
        let mut work = plan.work;
        for y in 0..viewport.height {
            for x in 0..viewport.width {
                let mut sum = LinearPixel::default();
                for sy in [0.25, 0.75] {
                    for sx in [0.25, 0.75] {
                        charge(&mut work, 1, &self.limits)?;
                        let point = Point {
                            x: viewport.origin.x + (x as f64 + sx) / viewport.scale,
                            y: viewport.origin.y + (y as f64 + sy) / viewport.scale,
                        };
                        let mut result = LinearPixel::default();
                        if plan.canvas.is_none_or(|canvas| contains(canvas, point)) {
                            for surface in &plan.surfaces {
                                charge(&mut work, 1, &self.limits)?;
                                if surface.bounds.is_none_or(|b| !b.contains(point)) {
                                    continue;
                                }
                                let Some(inverse) = surface.inverse else {
                                    continue;
                                };
                                let local = inverse.point(point);
                                valid_point(local)?;
                                // render_scene uses an unbounded sentinel, render_view uses content_size.
                                if surface.content.x.is_finite()
                                    && !contains(surface.content, local)
                                {
                                    continue;
                                }
                                if surface.clip.is_some_and(|clip| !contains(clip, local)) {
                                    continue;
                                }
                                let source = self
                                    .sample(surface.root, point, plan, &mut work)?
                                    .multiply(surface.opacity);
                                result = source.over(result);
                            }
                        }
                        sum = sum.add(result);
                    }
                }
                pixels.push(sum.multiply(0.25));
            }
        }
        Ok(Frame {
            revision,
            viewport,
            pixels,
            work,
            budget: self.budget.clone(),
            _memory: memory,
        })
    }
    fn validate_viewport(&self, viewport: Viewport) -> Result<usize, RenderError> {
        let count = (viewport.width as usize)
            .checked_mul(viewport.height as usize)
            .ok_or(RenderError::Limit("pixels"))?;
        if viewport.width == 0
            || viewport.height == 0
            || viewport.width > self.limits.max_dimension
            || viewport.height > self.limits.max_dimension
            || count > self.limits.max_pixels
            || !viewport.scale.is_finite()
            || viewport.scale <= 0.0
            || !viewport.origin.x.is_finite()
            || !viewport.origin.y.is_finite()
        {
            return Err(RenderError::Limit("viewport"));
        }
        if !(viewport.origin.x + viewport.width as f64 / viewport.scale).is_finite()
            || !(viewport.origin.y + viewport.height as f64 / viewport.scale).is_finite()
        {
            return Err(RenderError::Invalid);
        }
        Ok(count)
    }
    fn sample(
        &self,
        index: usize,
        world: Point,
        plan: &Plan<'_>,
        work: &mut u64,
    ) -> Result<LinearPixel, RenderError> {
        *work = work
            .checked_add(1)
            .ok_or(RenderError::Limit("sample work"))?;
        if *work > self.limits.max_work {
            return Err(RenderError::Limit("sample work"));
        }
        let node = &plan.nodes[index];
        if node.bounds.is_none_or(|b| !b.contains(world)) {
            return Ok(LinearPixel::default());
        }
        let Some(inverse) = node.inverse else {
            return Ok(LinearPixel::default());
        };
        let p = inverse.point(world);
        valid_point(p)?;
        if node.clip.is_some_and(|clip| !contains(clip, p)) {
            return Ok(LinearPixel::default());
        }
        let paint = match &node.paint {
            Paint::Group(children) => {
                let mut result = LinearPixel::default();
                for child in children {
                    result = self.sample(*child, world, plan, work)?.over(result);
                }
                result
            }
            Paint::Rect(rect, color) => {
                if contains(*rect, p) {
                    LinearPixel::from_color(*color)
                } else {
                    LinearPixel::default()
                }
            }
            Paint::Rounded(rect, radii, color) => {
                if rounded_contains(*rect, *radii, p) {
                    LinearPixel::from_color(*color)
                } else {
                    LinearPixel::default()
                }
            }
            Paint::Border(outer, radii, inner, inner_radii, color) => {
                if rounded_contains(*outer, *radii, p) && !rounded_contains(*inner, *inner_radii, p)
                {
                    LinearPixel::from_color(*color)
                } else {
                    LinearPixel::default()
                }
            }
            Paint::Linear(rect, start, end, stops) => {
                if !contains(*rect, p) {
                    LinearPixel::default()
                } else {
                    let dx = end.x - start.x;
                    let dy = end.y - start.y;
                    let length = dx.hypot(dy);
                    let t = ((p.x - start.x) * (dx / length) + (p.y - start.y) * (dy / length))
                        / length;
                    if t.is_nan() {
                        return Err(RenderError::Invalid);
                    }
                    gradient(stops, t)
                }
            }
            Paint::Radial(rect, center, inner, outer, stops) => {
                if contains(*rect, p) {
                    gradient(
                        stops,
                        ((p.x - center.x).hypot(p.y - center.y) - inner) / (outer - inner),
                    )
                } else {
                    LinearPixel::default()
                }
            }
            Paint::Path(path, color) => {
                if path.contains(p, work, &self.limits)? {
                    LinearPixel::from_color(*color)
                } else {
                    LinearPixel::default()
                }
            }
            Paint::Glyph(path, size, positions, color) => {
                let mut result = LinearPixel::default();
                for position in *positions {
                    *work = work.saturating_add(1);
                    if *work > self.limits.max_work {
                        return Err(RenderError::Limit("glyph work"));
                    }
                    let local = Point {
                        x: (p.x - position.x) / size,
                        y: (p.y - position.y) / size,
                    };
                    if path.contains(local, work, &self.limits)? {
                        result = LinearPixel::from_color(*color).over(result);
                    }
                }
                result
            }
        };
        Ok(paint.multiply(node.opacity))
    }
}
fn valid_opacity(value: f64) -> Result<(), RenderError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        Err(RenderError::Invalid)
    } else {
        Ok(())
    }
}
pub(crate) fn charge(
    work: &mut u64,
    amount: u64,
    limits: &RenderLimits,
) -> Result<(), RenderError> {
    *work = work.checked_add(amount).ok_or(RenderError::Limit("work"))?;
    if *work > limits.max_work {
        Err(RenderError::Limit("work"))
    } else {
        Ok(())
    }
}
fn valid_point(point: Point) -> Result<(), RenderError> {
    if !point.x.is_finite() || !point.y.is_finite() {
        Err(RenderError::Invalid)
    } else {
        Ok(())
    }
}
fn valid_radii(rect: Rect, radii: CornerRadii) -> Result<(), RenderError> {
    let corners = [
        radii.top_left,
        radii.top_right,
        radii.bottom_right,
        radii.bottom_left,
    ];
    for radius in corners {
        if !radius.width.is_finite()
            || !radius.height.is_finite()
            || radius.width < 0.0
            || radius.height < 0.0
            || (radius.width == 0.0) != (radius.height == 0.0)
        {
            return Err(RenderError::Invalid);
        }
    }
    if radii.top_left.width + radii.top_right.width > rect.width
        || radii.bottom_left.width + radii.bottom_right.width > rect.width
        || radii.top_left.height + radii.bottom_left.height > rect.height
        || radii.top_right.height + radii.bottom_right.height > rect.height
    {
        return Err(RenderError::Invalid);
    }
    Ok(())
}
fn valid_color(color: Color) -> Result<(), RenderError> {
    for value in [color.red, color.green, color.blue, color.alpha] {
        valid_opacity(value)?;
    }
    if color.red > color.alpha || color.green > color.alpha || color.blue > color.alpha {
        Err(RenderError::Invalid)
    } else {
        Ok(())
    }
}
fn valid_rect(rect: Rect) -> Result<(), RenderError> {
    if [
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        rect.x + rect.width,
        rect.y + rect.height,
    ]
    .iter()
    .any(|v| !v.is_finite())
        || rect.width < 0.0
        || rect.height < 0.0
    {
        Err(RenderError::Invalid)
    } else {
        Ok(())
    }
}
fn valid_stops(stops: &[GradientStop]) -> Result<(), RenderError> {
    if stops.len() < 2
        || stops.first().unwrap().offset != 0.0
        || stops.last().unwrap().offset != 1.0
    {
        return Err(RenderError::Invalid);
    }
    for stop in stops {
        valid_opacity(stop.offset)?;
        valid_color(stop.color)?;
    }
    if stops.windows(2).any(|pair| pair[0].offset > pair[1].offset) {
        return Err(RenderError::Invalid);
    }
    Ok(())
}
fn contains(rect: Rect, p: Point) -> bool {
    p.x >= rect.x && p.x < rect.x + rect.width && p.y >= rect.y && p.y < rect.y + rect.height
}
fn rounded_contains(rect: Rect, radii: CornerRadii, p: Point) -> bool {
    if !contains(rect, p) {
        return false;
    }
    let corners = [
        (
            radii.top_left,
            rect.x + radii.top_left.width,
            rect.y + radii.top_left.height,
            p.x < rect.x + radii.top_left.width && p.y < rect.y + radii.top_left.height,
        ),
        (
            radii.top_right,
            rect.x + rect.width - radii.top_right.width,
            rect.y + radii.top_right.height,
            p.x > rect.x + rect.width - radii.top_right.width
                && p.y < rect.y + radii.top_right.height,
        ),
        (
            radii.bottom_right,
            rect.x + rect.width - radii.bottom_right.width,
            rect.y + rect.height - radii.bottom_right.height,
            p.x > rect.x + rect.width - radii.bottom_right.width
                && p.y > rect.y + rect.height - radii.bottom_right.height,
        ),
        (
            radii.bottom_left,
            rect.x + radii.bottom_left.width,
            rect.y + rect.height - radii.bottom_left.height,
            p.x < rect.x + radii.bottom_left.width
                && p.y > rect.y + rect.height - radii.bottom_left.height,
        ),
    ];
    for (radius, cx, cy, in_corner) in corners {
        if in_corner && radius.width > 0.0 && radius.height > 0.0 {
            let x = (p.x - cx) / radius.width;
            let y = (p.y - cy) / radius.height;
            return x * x + y * y <= 1.0;
        }
    }
    true
}
fn inset(rect: Rect, radii: CornerRadii, width: f64) -> (Rect, CornerRadii) {
    let corner = |r: rui_protocol_core::model::Size| {
        let x = (r.width - width).max(0.0);
        let y = (r.height - width).max(0.0);
        if x == 0.0 || y == 0.0 {
            rui_protocol_core::model::Size {
                width: 0.0,
                height: 0.0,
            }
        } else {
            rui_protocol_core::model::Size {
                width: x,
                height: y,
            }
        }
    };
    (
        Rect {
            x: rect.x + width,
            y: rect.y + width,
            width: rect.width - 2.0 * width,
            height: rect.height - 2.0 * width,
        },
        CornerRadii {
            top_left: corner(radii.top_left),
            top_right: corner(radii.top_right),
            bottom_right: corner(radii.bottom_right),
            bottom_left: corner(radii.bottom_left),
        },
    )
}
fn gradient(stops: &[GradientStop], t: f64) -> LinearPixel {
    if t < 0.0 {
        return LinearPixel::from_color(stops[0].color);
    }
    if t > 1.0 {
        return LinearPixel::from_color(stops.last().unwrap().color);
    }
    let right = stops.partition_point(|stop| stop.offset <= t);
    if right == stops.len() {
        return LinearPixel::from_color(stops[right - 1].color);
    }
    let left = stops[right - 1];
    let right = stops[right];
    let fraction = (t - left.offset) / (right.offset - left.offset);
    LinearPixel::from_color(left.color)
        .multiply(1.0 - fraction)
        .add(LinearPixel::from_color(right.color).multiply(fraction))
}
