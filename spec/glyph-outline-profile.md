# Single-outline glyph profile

`glyph.outline-single.v1` is an optional, explicitly negotiated rendering
capability. It does not install fonts or infer text from geometry.

A GlyphRun using this profile references a kind-2, version-1 GlyphOutline
resource with FontOutline provenance. The exact RVP1 path describes one glyph
in normalized one-em coordinates, with x right, y down, and origin at its
baseline. Each glyph ID must be zero. For every host-shaped position `(x,y)`,
the renderer translates the outline by that position after scaling it by the
run's positive `size`. The run uses nonzero winding and its premultiplied color.
Glyphs are composited in transmitted order. Scalar clusters remain semantic
metadata and never trigger shaping, reflow, or spacing changes.

Distinct glyph outlines require distinct resources and runs. A producer can
lower a shaped font run into an ordered group of these runs while preserving
the original glyph positions. Shared outlines remain content-addressed.
Provenance must come from a trusted vector font producer, never raster tracing.

The receiver opts in through `ReceiverConfig::single_glyph_outlines` only after
the connection layer has negotiated this profile and installed a compatible
renderer. It still requires authorization, exact resource integrity, safe RVP1
decoding, and live resource pins before reporting Applied. Nonzero glyph IDs
and preinstalled font fingerprints remain unsupported by this profile. The
default receiver continues rejecting all glyph runs.
