# Pixel-aligned multicolor square borders

C2 adds opaque square borders with equal side widths and different side colors
for constrained controls. Four authored closed trapezoid paths preserve the
source outer/inner rectangles, clockwise side ordering and unpainted center.
Existing uniform borders keep their existing primitive. No profile extension or
raster fallback is needed: paths are ordinary authorized RVP resources, pinned
by the existing closure. The vector audit associates their new
`gsk-square-border` provenance event with the actual submitted BorderNode.

The declared subset has zero corner radii, equal finite widths no larger than
half either dimension, integer geometry and integer translations at scale 1.
Accumulated transforms are checked through all ancestors. Other widths, scales,
rotation, fractional coordinates and rounded multicolor borders remain partial.
Opaque paint is required before any resource allocation. Each accepted node
adds at most four five-command paths and five scene records under existing node,
record, resource and capture budgets. Clipping and overlap use existing primitives.

Fresh same-submitted-tree comparisons cover two widths, translation, rectangular
clipping and an opaque underlay visible through the center. All four pass the
unchanged GTK/reference criteria with zero bad interior pixels. A deliberately
omitted side fails. [The report](../fixtures/gtk/border-comparison.json) binds the
retained envelopes/manifests/vector bodies and environment. Native evidence
showed corner coverage differences beyond that tolerance for fractional scale
and unequal widths; those cases were excluded rather than broadening the claim.
Tests require their explicit decline, including ancestor scale and translucent paint.
Default Adwaita rounded clips, gradients and shadows remain gaps from the matrix.

```sh
cargo build --release -p rui-client-desktop --example render_capture --locked
python3 fixtures/gtk/border_check.py --xvfb /path/to/Xvfb --output /tmp/rui-border-NEW
python3 -m unittest adapters.gtk4.test_capture fixtures.gtk.test_negatives
cargo test -p rui-producer-sdk --test border_trace --test negative_trace --locked
```
