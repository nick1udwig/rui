use crate::renderer::{charge, RenderError, RenderLimits, ResourceProvider};
use cairo::{Context, Format, ImageSurface};
use rui_protocol_core::model::{FillRule, Point, ResourceId, StrokeCap, StrokeJoin};
use rui_protocol_core::resources::{
    resource_digest, validate_vector_bytes, MemoryBudget, MemoryReservation, ResourceKind,
    ResourceLimits, ResourceProvenance,
};

#[derive(Clone, Copy)]
enum Flat {
    Move(Point),
    Line(Point),
    Close,
}
struct Builder {
    commands: Vec<Flat>,
    limit: usize,
    tolerance: f64,
    depth_limit: usize,
    last: Point,
    start: Point,
    length: f64,
    edges: usize,
}
impl Builder {
    fn push(&mut self, command: Flat) -> Result<(), RenderError> {
        if self.commands.len() == self.limit {
            return Err(RenderError::Limit("flattened path segments"));
        }
        self.commands.push(command);
        Ok(())
    }
    fn line(&mut self, p: Point) -> Result<(), RenderError> {
        if p != self.last {
            self.length += (p.x - self.last.x).hypot(p.y - self.last.y);
            self.edges += 1;
            self.push(Flat::Line(p))?;
        }
        self.last = p;
        Ok(())
    }
    fn quadratic(&mut self, a: Point, b: Point, c: Point, depth: usize) -> Result<(), RenderError> {
        if distance(b, a, c) <= self.tolerance {
            return self.line(c);
        }
        if depth == self.depth_limit {
            return Err(RenderError::Limit("curve subdivision depth"));
        }
        let ab = mid(a, b);
        let bc = mid(b, c);
        let m = mid(ab, bc);
        self.quadratic(a, ab, m, depth + 1)?;
        self.quadratic(m, bc, c, depth + 1)
    }
    fn cubic(
        &mut self,
        a: Point,
        b: Point,
        c: Point,
        d: Point,
        depth: usize,
    ) -> Result<(), RenderError> {
        if distance(b, a, d).max(distance(c, a, d)) <= self.tolerance {
            return self.line(d);
        }
        if depth == self.depth_limit {
            return Err(RenderError::Limit("curve subdivision depth"));
        }
        let ab = mid(a, b);
        let bc = mid(b, c);
        let cd = mid(c, d);
        let abc = mid(ab, bc);
        let bcd = mid(bc, cd);
        let m = mid(abc, bcd);
        self.cubic(a, ab, abc, m, depth + 1)?;
        self.cubic(m, bcd, cd, d, depth + 1)
    }
}
fn mid(a: Point, b: Point) -> Point {
    Point {
        x: (a.x + b.x) / 2.0,
        y: (a.y + b.y) / 2.0,
    }
}
fn distance(p: Point, a: Point, b: Point) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length = dx * dx + dy * dy;
    let t = if length == 0.0 {
        0.0
    } else {
        ((p.x - a.x) * dx + (p.y - a.y) * dy) / length
    }
    .clamp(0.0, 1.0);
    (p.x - a.x - t * dx).hypot(p.y - a.y - t * dy)
}

pub(crate) struct StrokeStyle<'a> {
    pub width: f64,
    pub cap: StrokeCap,
    pub join: StrokeJoin,
    pub miter_limit: f64,
    pub dashes: &'a [f64],
    pub offset: f64,
}
pub(crate) struct PathMask {
    context: Context,
    bounds: (f64, f64, f64, f64),
    stroke: bool,
    pub work: u64,
    _memory: MemoryReservation,
}
impl PathMask {
    pub(crate) fn bounds(&self) -> (f64, f64, f64, f64) {
        self.bounds
    }
    pub fn contains(
        &self,
        p: Point,
        work: &mut u64,
        limits: &RenderLimits,
    ) -> Result<bool, RenderError> {
        let (x1, y1, x2, y2) = self.bounds;
        if p.x < x1 || p.x > x2 || p.y < y1 || p.y > y2 {
            return Ok(false);
        }
        *work = work
            .checked_add(self.work)
            .ok_or(RenderError::Limit("sample work"))?;
        if *work > limits.max_work {
            return Err(RenderError::Limit("sample work"));
        }
        if self.stroke {
            Ok(self.context.in_stroke(p.x, p.y)?)
        } else {
            Ok(self.context.in_fill(p.x, p.y)?)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn compile(
    id: ResourceId,
    kind: ResourceKind,
    provider: &impl ResourceProvider,
    fill_rule: FillRule,
    stroke: Option<StrokeStyle<'_>>,
    device_scale: f64,
    limits: &RenderLimits,
    budget: &MemoryBudget,
    work: &mut u64,
) -> Result<PathMask, RenderError> {
    let resource = provider.get(&id).ok_or(RenderError::MissingResource(id))?;
    if resource.bytes.len() > limits.max_resource_bytes {
        return Err(RenderError::Limit("resource bytes"));
    }
    let provenance = match kind {
        ResourceKind::VectorPath => ResourceProvenance::AuthoredVector,
        ResourceKind::GlyphOutline => ResourceProvenance::FontOutline,
    };
    if resource.offer.kind != kind
        || resource.offer.version != 1
        || resource.offer.provenance != provenance
        || resource.offer.total_bytes != resource.bytes.len() as u64
        || resource.offer.digest != id
    {
        return Err(RenderError::ResourceDescriptor);
    }
    charge(work, resource.bytes.len() as u64, limits)?;
    if resource_digest(kind, 1, provenance, resource.bytes) != id {
        return Err(RenderError::ResourceIntegrity);
    }
    let validation_limits = ResourceLimits {
        max_vector_commands: limits.max_path_commands,
        max_coordinate_abs: limits.max_path_coordinate,
        ..ResourceLimits::default()
    };
    validate_vector_bytes(resource.bytes, &validation_limits)?;
    if !device_scale.is_finite() {
        return Err(RenderError::Invalid);
    }
    if let Some(style) = &stroke {
        if !style.width.is_finite()
            || style.width <= 0.0
            || !style.miter_limit.is_finite()
            || style.miter_limit < 1.0
            || (style.join != StrokeJoin::Miter && style.miter_limit != 1.0)
            || !(style.width * style.miter_limit).is_finite()
            || !style.offset.is_finite()
            || style.offset < 0.0
            || !style.dashes.len().is_multiple_of(2)
            || style
                .dashes
                .iter()
                .any(|value| !value.is_finite() || *value <= 0.0)
        {
            return Err(RenderError::Invalid);
        }
        if style.width > limits.max_stroke_width
            || style.miter_limit > limits.max_miter_limit
            || style.dashes.len() > limits.max_dash_lengths
        {
            return Err(RenderError::Limit("stroke options"));
        }
        let period: f64 = style.dashes.iter().sum();
        if !period.is_finite()
            || (style.dashes.is_empty() && style.offset != 0.0)
            || (!style.dashes.is_empty() && style.offset >= period)
        {
            return Err(RenderError::Invalid);
        }
    }
    let flat_bytes = limits
        .max_flat_segments
        .checked_mul(std::mem::size_of::<Flat>())
        .and_then(|n| n.checked_add(1024))
        .ok_or(RenderError::Limit("path allocation"))?;
    let _flat_memory = budget.reserve(flat_bytes)?;
    let mut commands = Vec::new();
    commands
        .try_reserve_exact(limits.max_flat_segments)
        .map_err(|_| RenderError::Allocation)?;
    let mut builder = Builder {
        commands,
        limit: limits.max_flat_segments,
        tolerance: limits.curve_tolerance_pixels / device_scale.max(1.0),
        depth_limit: limits.max_curve_depth,
        last: Point { x: 0.0, y: 0.0 },
        start: Point { x: 0.0, y: 0.0 },
        length: 0.0,
        edges: 0,
    };
    let mut position = 8usize;
    let point = |position: &mut usize| -> Point {
        let x = f64::from_be_bytes(resource.bytes[*position..*position + 8].try_into().unwrap());
        *position += 8;
        let y = f64::from_be_bytes(resource.bytes[*position..*position + 8].try_into().unwrap());
        *position += 8;
        Point { x, y }
    };
    let count = u32::from_be_bytes(resource.bytes[4..8].try_into().unwrap());
    for _ in 0..count {
        let opcode = resource.bytes[position];
        position += 1;
        match opcode {
            1 => {
                let p = point(&mut position);
                builder.push(Flat::Move(p))?;
                builder.last = p;
                builder.start = p;
            }
            2 => {
                let p = point(&mut position);
                builder.line(p)?;
            }
            3 => {
                let b = point(&mut position);
                let c = point(&mut position);
                builder.quadratic(builder.last, b, c, 0)?;
            }
            4 => {
                let b = point(&mut position);
                let c = point(&mut position);
                let d = point(&mut position);
                builder.cubic(builder.last, b, c, d, 0)?;
            }
            5 => {
                builder.length +=
                    (builder.start.x - builder.last.x).hypot(builder.start.y - builder.last.y);
                builder.edges += usize::from(builder.last != builder.start);
                builder.push(Flat::Close)?;
                builder.last = builder.start;
            }
            _ => unreachable!("validated vector opcode"),
        }
    }
    charge(work, builder.commands.len() as u64, limits)?;
    let dash_intervals = if let Some(style) = &stroke {
        if style.width > limits.max_stroke_width {
            return Err(RenderError::Limit("stroke width"));
        }
        if style.dashes.is_empty() {
            0
        } else {
            let minimum = style.dashes.iter().copied().fold(f64::INFINITY, f64::min);
            let intervals = (builder.length / minimum).ceil();
            if !intervals.is_finite() || intervals > limits.max_dash_intervals as f64 {
                return Err(RenderError::Limit("dash intervals"));
            }
            intervals as usize + count as usize
        }
    } else {
        0
    };
    let memory = budget.reserve(
        builder
            .commands
            .len()
            .saturating_add(dash_intervals)
            .saturating_mul(256)
            .saturating_add(16_384),
    )?;
    let surface = ImageSurface::create(Format::A8, 1, 1)?;
    let context = Context::new(&surface)?;
    context.set_tolerance(limits.curve_tolerance_pixels);
    context.set_fill_rule(match fill_rule {
        FillRule::Nonzero => cairo::FillRule::Winding,
        FillRule::EvenOdd => cairo::FillRule::EvenOdd,
    });
    // Exclude entirely zero-length contours: Cairo's round zero-length stroke
    // dot is outside this profile. Delay MoveTo until the first real line.
    let mut pending = Point { x: 0.0, y: 0.0 };
    let mut started = false;
    for command in &builder.commands {
        match *command {
            Flat::Move(p) => {
                pending = p;
                started = false;
            }
            Flat::Line(p) => {
                if !started {
                    context.move_to(pending.x, pending.y);
                    started = true;
                }
                context.line_to(p.x, p.y);
            }
            Flat::Close => {
                if started {
                    context.close_path();
                }
                started = false;
            }
        }
    }
    if let Some(style) = &stroke {
        context.set_line_width(style.width);
        context.set_miter_limit(style.miter_limit);
        context.set_line_cap(match style.cap {
            StrokeCap::Butt => cairo::LineCap::Butt,
            StrokeCap::Round => cairo::LineCap::Round,
            StrokeCap::Square => cairo::LineCap::Square,
        });
        context.set_line_join(match style.join {
            StrokeJoin::Miter => cairo::LineJoin::Miter,
            StrokeJoin::Round => cairo::LineJoin::Round,
            StrokeJoin::Bevel => cairo::LineJoin::Bevel,
        });
        context.set_dash(style.dashes, style.offset);
    }
    context.status()?;
    let bounds = if stroke.is_some() {
        context.stroke_extents()?
    } else {
        context.fill_extents()?
    };
    Ok(PathMask {
        context,
        bounds,
        stroke: stroke.is_some(),
        work: builder.edges.saturating_add(dash_intervals).max(1) as u64,
        _memory: memory,
    })
}
