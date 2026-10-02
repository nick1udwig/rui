# Pinned CPU reference renderer

`renderer.cpu-samples2x2.cairo1184.v1` is a local reference rasterization profile
for the retained scene semantics in [rendering-profile.md](rendering-profile.md).
It implements all nine primitive forms. It does not introduce raster resources
or wire image payloads. Its PPM and PAM files are local test artifacts.

The implementation is `rui-client-desktop`. The Rust Cairo binding is pinned to
`cairo-rs 0.20.12`, and the renderer refuses any native Cairo version other than
`1.18.4`. Reference artifacts currently use the repository's x86-64 Linux/Rust
environment. Binary64 arithmetic and the system math library are part of that
environment: this is not a bit-identical claim for other architectures or math
libraries. There is no GPU, font discovery, text shaping, layout or application
state in this renderer.

## Sampling and linear composition

An output viewport specifies dimensions in device pixels, a host logical origin,
and a positive scale. Pixel `(x,y)` has exactly four samples at device positions
`(x+0.25,y+0.25)`, `(x+0.75,y+0.25)`, `(x+0.25,y+0.75)` and
`(x+0.75,y+0.75)`. Divide those positions by scale and add the viewport origin
to obtain host coordinates. Observer zoom changes only this mapping.

Each sample evaluates the entire scene in premultiplied linear sRGB. Source-over
uses the equations in the scene vocabulary. Group children composite in their
transmitted order into transparent content; the group's opacity multiplies that
result once. Clips test the group's local point before descendants contribute.
Ancestor transforms compose with descendant transforms. Singular transforms
contribute no coverage. Nonfinite transform evaluation fails the entire render.

After all surfaces composite, average the four premultiplied linear RGBA values.
No Cairo color blending, gamma interpolation or per-child opacity approximation
is used. A returned `Frame` retains those binary64 values for independent pixel
assertions. The default background is transparent.

PAM export unpremultiplies the averaged RGB, applies the sRGB display transfer
function (`12.92*c` below or at `0.0031308`, otherwise
`1.055*c^(1/2.4)-0.055`), and rounds each component to the nearest byte with
`floor(255*c+0.5)`. Alpha uses the same rounding without a transfer function.
Transparent output is `(0,0,0,0)`. PPM export first composites over a caller's
opaque linear color and then applies that transfer function. Export streams one
budgeted row at a time.

This four-sample profile deliberately fixes coverage rather than claiming exact
area antialiasing. Geometry and pixel tests assert mathematical coverage at
those sample locations; they do not compare two invocations of this renderer.

## Geometry, paths and glyphs

Rectangles, rounded elliptical corners, inward border rings and gradients use
the scene specification directly. Rectangles are half-open; the curved outer
boundary of an elliptical corner is included. Gradient duplicate stops,
premultiplied interpolation and PAD behavior preserve transmitted order.

RVP1 resources are independently checked for descriptor/body length, kind,
provenance, exact digest and safe path structure before path construction.
Quadratic and cubic curves are flattened by recursive de Casteljau subdivision
at the midpoint. A segment is flat when every control point is within the
configured tolerance of its endpoint **segment**. The default tolerance is
`1/256` device pixel, divided by a conservative L1 bound on the composed linear
transform and viewport scale (the divisor is at least one). Flat line segments
are passed to Cairo with an identity matrix. Cairo 1.18.4 `in_fill`/`in_stroke`
defines exact boundary membership, winding, stroke joins, caps and dash seams
for this flattened path. Cairo never composites colors. Native fixed-point
geometry and its boundary conventions are part of the pinned profile.

Entirely zero-length contours are removed before Cairo construction, including
Round-cap contours, which therefore paint no independent dot. Open fill
contours close implicitly. Nonzero stroke pieces paint as one union. Dash phase
resets at MoveTo; painted pieces straddling a closed seam join there. Tests
assert the seam's outside miter corner independently of rendered baselines.

Glyphs require [glyph.outline-single.v1](glyph-outline-profile.md). The renderer
checks every ID is zero, scales the normalized outline by the host run size,
and translates it by each transmitted host-shaped position. Overlapping glyph
instances composite in transmitted order, including when their color is
translucent. Scalar clusters never trigger shaping. Preinstalled font
fingerprints, missing outlines and other glyph IDs are explicit errors. The
reference image's authored vector glyph is a fixture, not a replacement for
an unavailable font. GTK capture tests instead use outlines extracted from the
source's actual scaled Pango/Cairo font.

## Surfaces and explicit failures

A surface maps content to desktop coordinates by
`T(desktop_rect.x,desktop_rect.y) * surface.transform`. Its local content region
is `[0,content_size.width) x [0,content_size.height)`, intersected with its optional
local clip. `desktop_rect.width/height` remain placement/bounds metadata; they
do not implicitly rescale or crop content. A parent surface establishes topology,
not another relative translation. Mapped surfaces composite by ascending
`(stacking_order,id)`; each surface's opacity applies to its isolated root result.
A session canvas clips the desktop. Unmapped surfaces contribute no coverage.

Partial or Unsupported mapped surfaces fail the entire render. Missing resources,
bad descriptors, unavailable fonts, unsupported cursor appearances, malformed
geometry and exhausted limits also fail without returning a frame. Hidden
cursors are supported; this profile has no default system-cursor asset or
cursor-outline paint convention, so visible cursors require a future profile.
Nothing silently disappears under a successful Complete render.

`ResourceProvider` must return only authorized resources for the current view.
Its implementation for `ViewReceiver` borrows verified offers and bodies under
the receiver's binding, closure and authorization checks. A renderer borrows
the committed state during the entire render, retaining the receiver's resource
pins. Producing a frame does not itself send PresentedAck: the display integration
must acknowledge after it presents the corresponding applied revision.

## Bounds and memory

`RenderLimits` are local ceilings. Defaults bound the frame to 1,048,576 pixels
and each dimension to 4096, scene nodes/edges/surfaces to 10,000, scene depth to
128, resource bytes to 16 MiB, vector commands to 4096, flattened commands to
16,384 per path, subdivision depth to 20, gradient stops and glyphs per run to
4096, dash lengths to 1024 and dash intervals to 4096 per path. Stroke widths
are at most 4096, miter ratios at most 32, resource coordinates at most 1,000,000,
and the composed transform's L1 linear bound is at most 1,000,000. Unsupported
or out-of-profile values fail explicitly; callers may lower ceilings.

The default work ceiling is 200,000,000 units. Preparation charges resource hash
bytes, retained-node visits, stops, glyph positions and flattened commands.
Sampling charges sample/surface/node visits and conservatively charges path
edges/dash intervals per membership query plus glyph-instance visits. These
ceilings limit work rather than promise a frame-rate or real-time deadline.

The renderer must share the receiver's `MemoryBudget`. It reserves before frame,
plan, child-index, temporary flattened-path and export-row allocations. Frame
and path reservations live as long as their allocations; failed renders release
their temporary reservations. The ledger charges a conservative 256 bytes per
flat command/dash interval plus 16 KiB per native Cairo path/context. Native Cairo
allocation is not instrumented byte by byte or sandboxed in this implementation;
its accepted command/coordinate/dash complexity is explicitly bounded. The ledger
is an allocation policy, not an RSS proof. Returned frames own their reservations
and are not clonable. Resource and scene input allocations stay owned and charged
by the caller/receiver. Recursive group evaluation avoids full-size intermediate
frame buffers.
Conservative desktop bounds cull offscreen subtrees before exact geometry tests.
Bounds include a floating-point error margin; unstable inverse transforms disable
culling rather than discard uncertain coverage.

## Local tools and validation

```
cargo test -p rui-client-desktop --offline --locked
cargo run -p rui-client-desktop --example reference --offline --locked -- fixtures/render
cargo run -p rui-client-desktop --bin render-snapshot --offline --locked -- \
  snapshot.pb descriptors.tsv resources output.pam 320 180
cargo run -p rui-client-desktop --example render_capture --offline --locked -- \
  capture.json output.pam
```

The raw-snapshot tool accepts one exact Snapshot Protobuf payload and a bounded
local descriptor list: each line is `digest_lowercase_hex kind version provenance`.
Bodies are `<digest>.rvp` in the specified directory. The tool checks integrity,
safe paths and snapshot resource closure before rendering. These are caller-trusted
local fixture descriptors, not an authenticated broker or resource grant protocol.
The capture example consumes the SDK's same-node GTK diagnostic fixture and uses
the receiver's shared ledger and authorized cache.
Its `--scene` option compares one explicitly Complete captured content root at
local coordinates. This does not claim the entire multi-surface view is Complete;
in particular an uncaptured partial parent remains an error for whole-view rendering.

`fixtures/render/reference.ppm` and `reference.pam` exercise all nine forms,
gradient interpolation, native path coverage, single outlines and popup surface
composition. Tests independently check fixed coverage, linear source-over and
display transfer, isolated opacity, transforms/clips, surface placement, rounded
geometry, gradients, fill rules, stroke caps/joins/dashes, glyph positions/overlap,
explicit errors and allocation/work cleanup. This renderer alone does not satisfy
the RFC's multi-client control/reconnect gates, GTK fidelity comparison gate,
desktop integration or hardened graphics sandbox requirement.
