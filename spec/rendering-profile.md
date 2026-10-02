# RUI initial retained scene vocabulary

This document defines the mathematical paint semantics of the nine initial
`SceneNode` primitives in `rui.proto`. It defines a bounded model vocabulary,
not a renderer or a pixel comparison claim. A renderer profile must additionally
pin rasterization, antialiasing and exact font handling. Unsupported source
features require explicit coverage gaps or an equivalence-preserving lowering.

## Coordinates, composition and resources

Coordinates are binary64 host logical units, with x increasing right and y
increasing down. All transmitted numeric values must be finite. Negative zero
normalizes to positive zero. The default absolute geometry ceiling is `1e9`;
rectangles have nonnegative dimensions and bounded far edges. Rectangles are
half-open. A zero-area paint rectangle contributes no coverage.

The affine matrix maps local to parent coordinates as
`x' = xx*x + xy*y + tx`, `y' = yx*x + yy*y + ty`. Primitive geometry, clips,
stroke widths and dash lengths are specified in local coordinates. Apply the
node's transform after constructing its local paint region; strokes scale with
that transform. Device-pixel hairlines and non-scaling strokes are unsupported.

Colors use premultiplied **linear sRGB**. Every channel is in `[0,1]`, and each
RGB channel is at most alpha. Color-space conversion, HDR values and other blend
modes are unsupported. Source-over composition is:

```
out.rgb = source.rgb + destination.rgb * (1 - source.alpha)
out.alpha = source.alpha + destination.alpha * (1 - source.alpha)
```

Opacity is in `[0,1]` and multiplies both premultiplied RGB and alpha. A `Group`
composites its ordered children, first to last, onto transparent content before
applying its own clip and opacity. This is an isolated group: applying group
opacity separately to overlapping children is not equivalent. Clip rectangles
restrict the local paint region. Each non-Group primitive is a leaf.

`SolidRect` paints its rectangle with one color. `FillPath` paints the interior
of an exact validated vector resource using the specified nonzero winding or
even-odd rule. Open contours are implicitly closed for filling. Resource bytes,
contour structure and safe-decoding ceilings are defined in
[resource-format.md](resource-format.md). No primitive permits external fetching,
executable code or raster payloads. Both `FillPath` and `StrokePath` implicitly
require their referenced resource, even when `SceneNode.resource_refs` omits it.
The receiver must check resource kind, authorization, safe decoding and pins.

`GlyphRun` retains source-shaped glyph IDs, positions, size, color and optional
scalar clusters; clients must not reshape or reflow it. A font fingerprint must
identify the exact verified preinstalled font. This model does not implement a
font loader, glyph rasterizer, multi-glyph outline bundle or variation axes.
The current individual `GlyphOutline` resource format alone does not define a
multi-glyph ID mapping. The optional [single-outline glyph profile](glyph-outline-profile.md)
defines normalized outlines with glyph ID zero and explicit receiver opt-in.
Unsupported glyph resource or font handling must remain an explicit
error. The nine-primitive schema does not establish renderer support for fonts.

## Rounded rectangles and inward borders

`CornerRadii` contains four explicit elliptical radius pairs in clockwise order:
top-left, top-right, bottom-right, bottom-left. `Size.width` is x radius and
`Size.height` is y radius. Each corner is either `(0,0)`, a square corner, or has
both radii strictly positive. Mixed-zero pairs and negative/nonfinite radii are
invalid. Quarter ellipses join the rectangle's straight edges tangentially.

The two x radii along each horizontal edge may sum to at most the rectangle's
width. The two y radii along each vertical edge may sum to at most its height.
There is no CSS-style automatic radius rescaling. A `RoundedRect` paints this
shape with its single color.

`Border` is a single uniform, solid-color inward ring. Its width `w` satisfies
`0 <= w <= min(rect.width, rect.height)/2`. Width zero is allowed and paints
nothing. Per-edge widths, per-edge colors, outside alignment and border styles
are unsupported.

Construct the inner rectangle by insetting every edge by `w`. Subtract `w` from
each outer radius component and clamp the result at zero. If either component
of a resulting inner corner is zero, normalize **both** to zero, producing a
square inner corner. The resulting radii must satisfy the same edge-sum rules
for the inner rectangle; otherwise reject the border. Do not silently scale
inner radii. Paint the outer rounded region minus the inner rounded region
exactly once. A zero-area inner region has no interior to subtract. This is a
geometric ring, not an outer fill followed by transparent source-over paint.

## Linear and concentric radial gradients

Both gradients paint only their explicit rectangle. A linear gradient has
distinct start/end points. Its span must be finite and nonzero. For local point
`p`, with direction `d = end - start`, its parameter is
`t = dot(p-start, d) / dot(d,d)`. Implementations may evaluate an equivalent
numerically stable normalized-direction form.

A radial gradient consists of concentric circles at `center`, with
`0 <= inner_radius < outer_radius`. All four outer-circle axis extrema must
remain inside the negotiated absolute geometry ceiling. Its parameter is
`t = (distance(p,center)-inner_radius)/(outer_radius-inner_radius)`.
Elliptical radial shapes can be obtained by transforming this circular shape.
Focal offsets, two different centers and conic gradients are unsupported.

`spread` must explicitly be `PAD`. Repeat, reflect and implicit spread defaults
are unsupported. Each gradient has 2 through 4096 stops by default, with a
configurable local downward stop limit. Offsets are finite values in `[0,1]`. The first
offset is exactly zero and the last exactly one. Offsets are nondecreasing;
unsorted stops are rejected rather than reordered.

Duplicate offsets are allowed and preserve transmitted order. At an exact stop
offset, select the **last** color at that offset. Between distinct offsets `a`
and `b`, interpolate from the last color at `a` to the first color at `b`,
linearly in all four premultiplied linear-sRGB components. Thus duplicate stops
produce right-continuous hard edges; there is no division by a zero interval.
For `t < 0`, use the first transmitted stop's color; for `t > 1`, use the last
transmitted stop's color. Do not interpolate straight/unpremultiplied RGB or
apply gamma to interpolation.

## Vector path strokes

`StrokePath` paints a validated vector path resource with one premultiplied
color and a strictly positive finite local width. Fill rules do not apply to
stroke coverage. The stroke region is centered on the path, with half the width
on either side. Overlapping stroke pieces and self-intersections form a union
that is painted once; overlap must not multiply opacity.

Supported caps are `BUTT`, `ROUND_CAP` and `SQUARE`. A butt cap ends at the path
endpoint's perpendicular line. A round cap adds a semicircle of radius half the
width. A square cap extends by half the width along the tangent. Caps apply to
open contours and individual visible dash fragments. A closed undashed contour
joins its final and first segments and has no endpoint caps. Zero-length
segments consume no arc length and contribute no independent painted dot;
an entirely zero-length contour paints nothing. Join directions use the nearest
nonzero limiting segment tangents.

Supported joins are `MITER`, `ROUND_JOIN` and `BEVEL`. A bevel joins the outside
offset edges by a straight segment. A round join uses the outside circular arc
of radius half the width. A miter extends the outside offset edges to their
intersection. `miter_limit` is the maximum ratio of miter-tip distance from the
vertex to half the stroke width. Ratios at the limit retain the miter; ratios
above it fall back to bevel. Reversals with no finite outside intersection also
fall back to bevel. The limit is finite, at least one and at most 32 by default;
the upper bound is a configurable local downward ceiling. For non-Miter joins the wire value must
be exactly one; ignored alternative miter configurations are rejected. The
product of width and miter limit must remain finite.

An empty dash list is a solid stroke and requires offset zero. A nonempty dash
list has an even number of strictly positive finite lengths, at most 1024 by
default. Lengths alternate painted and unpainted intervals, starting painted.
Their finite sum is the period and must not exceed the geometry ceiling. Offset
is normalized on the wire: `0 <= dash_offset < period`. Negative offsets, zero
lengths, odd lists and offsets requiring implicit modulo conversion are rejected.

For each contour, start at `dash_offset` units already consumed in the repeated
pattern and advance by geometric path arc length, including an explicit closing
segment. Interval endpoints belong to the next interval. Reset the phase at
each MoveTo. Apply the selected cap at each dash fragment's exposed ends. On a
closed contour, painted fragments on both sides of its start/end seam merge
and use a join at that seam. Other dash boundaries use caps. The path resource
contains geometry only; arbitrary stroke shaders and toolkit style objects are
unsupported.

## Required vocabulary and allocation bounds

Scene primitive tags 10 through 18 name the nine forms; tag 19 remains unknown
required vocabulary. Unknown primitive tags below 128 reject the publication,
including when a known primitive also occurs. New primitive option messages
also reserve tags below 128 for required behavior. Optional metadata uses tags
128 and above and cannot change these semantics. Unknown enum values reject.

The allocation-free wire scan counts all stop and dash entries through each
Scene/Record/Mutation wrapper, including repeated singular message fields whose
values merge or replace prior data. Packed and unpacked dash encodings share the
same limit. It checks lengths and decoded allocation budgets before Protobuf
allocation; the typed reducer then validates numeric and structural semantics.
These stop, dash and miter ceilings are local `Limits` settings; the session
`ReceiverLimits` schema does not yet negotiate them on the wire.
Retained estimates account actual String/Vec capacities, including capacity
retained by Protobuf merges, and conservative tree-node overhead. Resources,
publication bytes and staged/committed state must use the same memory ledger.
Malformed geometry, unsupported options, missing resource closure or a failed
budget reservation cannot publish a partial state.
