# Ordered segment geometry profile

`geometry.segments.v1` adds the `SegmentBatch` scene primitive at tag 19.
Producers that emit this primitive must require the profile during session negotiation and in their publications.
Clients without this profile can continue to use other scene primitives.

`data` is a packed array of float64 values.
Each eight-value tuple contains `x1, y1, x2, y2, red, green, blue, alpha`.
Colors use the existing premultiplied linear-sRGB rules.
Coordinates use the existing finite geometry limits.
A batch contains zero to 2,048 independent segments, in paint order.
It has no scene children and needs no resource objects.

`width` must be finite and positive.
`minimum_length` must be finite and nonnegative.
Both use scene coordinates and the existing geometry limits.
For each segment, compute `dx = x2 - x1`, `dy = y2 - y1`, and `length = hypot(dx, dy)`.
The local rectangle is `(0, -width / 2, max(length, minimum_length), width)`.
Its transform has columns `(dx / length, dy / length)` and `(-dy / length, dx / length)`, with translation `(x1, y1)`.
When the length is zero, use identity rotation.
This defines butt ends, with a minimum length extending forward from the start point.
It does not define connected-path joins.

Paint segments in order into an isolated group, then apply the scene node's opacity and clip with the existing group rules.
Apply ancestor and scene transforms before the per-segment transform.
The geometry representation preserves float64 endpoints and colors without quantization.
Floating-point evaluation and rasterizer rules still determine boundary coverage.

Receivers validate packed and unpacked scalar counts before protobuf allocation, including repeated merged messages.
They reject partial tuples, unknown required fields, invalid colors, invalid dimensions, and out-of-range coordinates or derived lengths.
Renderer work and memory budgets include the expanded segments.
The current browser and native renderers lower segments to their existing rectangle paths.

The game producer uses width `1.2` and minimum length `0.8`, as its previous beam projection did.
The primitive is also suitable for line plots, wireframes, and independent drawing strokes.
It does not depend on game simulation, MAME, or object identity.
