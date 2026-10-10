# Browser rendering optimization (P4)

The measured small browser workload spent more time in composition than state
export or preparation. This series reuses the linear frame and ImageData buffer,
keys coverage by geometry rather than paint, and computes gradient paint without
allocating a point/color object per pixel. It still repaints the complete frame.
No damage tracking, worker, OffscreenCanvas or approximate compositing is added.
Native Cairo rendering retains its existing separate contract.

Coverage keys include transforms, clips, bounds, immutable path/font digest,
glyph positions and stroke geometry. They exclude colors, gradient stops and
semantic scalar clusters, which do not affect this renderer's coverage. The
frame clears before composition, so removed or moved objects leave no stale
pixels. Isolated opacity still uses its complete intermediate layer.

The 64 MiB default ownership ceiling is unchanged. Frame replacement accounts
for overlap with the old buffers. Compiled graph metadata, coverage key strings
and map overhead now carry conservative charges as well as pixels and paths.
Scope reset/disconnect clears derived JavaScript state, coverage, paths, pixels
and reusable buffers. Rust retains its separately guarded reconnect state and
exact unresolved commands. Custom renderers receive an optional `reset()` hook
and must clear their own scope-bound data. Caller-retained state/resource copies
remain caller-owned. Counters measure owned references, not browser GC capacity,
GPU/backend internals or physical process RSS.

`CanvasRenderer`'s fourth constructor argument `false` selects the previous
allocation and scalar-paint path. CI compares all pixels against that path in
25 changing color/geometry/clip/opacity cases in Chromium 153, Firefox 155 and
Playwright WebKit 26.6. Other regressions cover removal, same ID after reset,
transforms, resize, repeated paint-only updates, memory rejection and disposal.
The three engines retain their existing vector/glyph coverage tolerance.

On the Intel i7-1280P, system Chromium 151, the separate 256×256 synthetic
8-record draw benchmark runs 20 warmups and 600 measured frames, three times.
Fresh-render pixel comparisons occur before measurement. Draw p95 falls from
5.0–5.3 ms to 3.4–3.5 ms (about 31–34%). Its optimized lifetime creates one
linear frame and one output buffer, builds eight masks and reuses them 2,572
times. Owned high-water is 3,032,606 bytes under an 8 MiB fixture ceiling;
disposal returns owned bytes to zero. The raw
[before](../fixtures/performance/renderer-p4-before/chromium-1.json) and
[after](../fixtures/performance/renderer-p4-after/chromium-1.json) reports include
samples, stage distributions, hardware and source hashes. This synthetic
workload is separate from the 10,000-record native load fixture.

Reproduce after building TypeScript, with no concurrent builds or tests:

```sh
RUI_RENDER_OUTPUT=/tmp/rui-render-NEW node client-browser/tests/measure.mjs
```

Use `RUI_WASM_CHROMIUM` to choose the same owned browser executable. Full demo
latency uses `demos/web/performance.py` and actual following visible animation
frames. Draw-only improvement does not establish a 100 ms end-to-end or general
60 Hz claim. The first P4 series leaves generation-bound state export, damage
and the large native render cost available for subsequent measured optimization.

The real demo's three 200-action runs per origin are retained as
[before](../fixtures/performance/browser-p4-before/summary.json) and
[after](../fixtures/performance/browser-p4-after/summary.json). Warm source-action
harness p95 is 152.9–153.8 ms after versus 153.8–155.3 ms before; browser-action
p95 varies from 134.1–186.5 ms after versus 134.9–183.4 ms before. This does not
establish a consistent end-to-end gain. The client command-to-actual-following-
frame and local stage samples remain separate in each raw report.
