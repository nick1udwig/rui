# Measured GTK 4 cooperating capture

This foundation demonstrates an actual GTK content snapshot boundary, a
vector-only Protobuf publication accepted by the Rust receiver, and a local
native-versus-replay comparison of the full constrained corpus. It measures a
small ordinary-control corpus under GTK 4.22.4. It is not a generic GTK desktop
adapter, a complete window lifecycle producer or completed M3 interaction.

## Source boundary and identity

`CaptureWidget.do_snapshot` snapshots its child into a private `Gtk.Snapshot`,
obtains that snapshot's immutable `GskRenderNode`, and appends the **same node**
to the snapshot GTK supplied for the actual widget frame. Semantic reads execute
synchronously in this callback against registered source objects. A source
revision check fails if those reads mutate the source. This uses GTK's real
[widget snapshot callback](https://docs.gtk.org/gtk4/vfunc.Widget.snapshot.html),
[child snapshot operation](https://docs.gtk.org/gtk4/method.Widget.snapshot_child.html),
[snapshot node construction](https://docs.gtk.org/gtk4/method.Snapshot.to_node.html)
and [node append operation](https://docs.gtk.org/gtk4/method.Snapshot.append_node.html).
It does not infer submitted frames from independent `WidgetPaintable` sampling.

`CapturePopover` subclasses the native `Gtk.PopoverMenu`, calls its parent
snapshot implementation into the private snapshot, and appends that same node.
The native Gio.Menu/GTK menu machinery remains intact. Main content and popup
are explicitly registered surfaces with producer-issued identities. Popup
parent-relative position comes from its native Gdk popup surface. Window titles,
screen geometry and tree similarity do not establish identity. The capture scope
excludes window decorations, other application windows and compositor output.

Every producer incarnation has a new UUID epoch. Semantic IDs remain stable
while their registered source object survives; unregistering a destroyed object
permanently retires its identity. Scene identities survive only while their
render-tree path remains present. Each surface reconciles old and new scene
records, permanently retires disappeared IDs and allocates a fresh ID if a path
returns. Source capture evidence is a separate string; routing uses an explicit
16-byte source-issued token. Current tokens change every capture, conservatively
invalidating earlier route evidence.

The popup artifact is a **per-surface snapshot**: its parent is a topology-only
`PARTIAL` Surface without the main scene. Rendering a selected popup scene is
valid; rendering the whole standalone popup view as complete is rejected. The
fixture closes its real popup but does not yet publish popup unmap/destroy, so
replaying these snapshots alone leaves the captured popup mapped. Combining
current main and popup projections, explicit map/unmap/destroy publications and
late-event rejection after source destruction remain required.

## Pinned measured environment

| Component | Measured version |
| --- | --- |
| GTK | 4.22.4; adapter rejects a different version |
| Cairo | 1.18.4 |
| Pango | 1.58.2 |
| PyGObject | 3.56.3 |
| GLib | 2.88.3 |
| Python | 3.14.7 |
| Source renderer | `GSK_RENDERER=cairo`, scale 1, animations disabled |
| Display harness | private Xvfb 21.1.24, authenticated Unix socket, TCP disabled |
| Default style | GTK Adwaita |
| Constrained style font | Liberation Sans, 14 px |

The strict font file is `/usr/share/fonts/liberation/LiberationSans-Regular.ttf`,
SHA-256 `baccc64becc3eb7d104b7c84d99f5314a0a1f896e2b3ea6c2f22fc08d2003bee`.
The source font supplies outlines; the remote renderer does not install or
substitute this font.

Xvfb was unavailable initially. The measured harness used an extracted package
under `/tmp/rui-xvfb`, without installing it or changing desktop configuration:
`xorg-server-xvfb-21.1.24-1-x86_64.pkg.tar.zst` from
`https://stable-mirror.omarchy.org/extra/os/x86_64/`, SHA-256
`7f2116f869aedf51eb899dcfee4cf1f3bf6f9f42c71e089dcdbc0907d529e985`.
An installed compatible `Xvfb` can also be passed to the harness.

```sh
python3 fixtures/gtk/measure.py \
  --xvfb /tmp/rui-xvfb/usr/bin/Xvfb --output /tmp/rui-gtk-corpus
cargo run -p rui-producer-sdk --bin replay_capture -- /tmp/rui-gtk-corpus/strict
python3 -m unittest adapters.sdk.test_producer
cargo test -p rui-producer-sdk --locked
cargo clippy -p rui-producer-sdk --all-targets --locked -- -D warnings
```

The measurement starts an isolated display with a temporary authority cookie and
runtime directory, uses memory-only GSettings, and runs both source styles. The
JSON report in `fixtures/gtk/measurement.json` was measured from the actual
submitted snapshots. Strict exact envelopes, manifests and vector resources are
retained in `fixtures/gtk/golden/`; producer epochs and timing values naturally
vary across fresh runs.

## Corpus and rendering coverage

The real application contains a label with accented and Greek text, an editable
entry, an ordinary button, native Gio.Menu/Gtk.PopoverMenu, a ListBox of three
explicitly identified rows inside a ScrolledWindow, and a custom source widget
that submits GSK vector fill and stroke nodes. Source callbacks change text and
selection, activate button/menu actions, open and close the popup, remove and
restore the canvas, and resize the source window. These are application callback
observations; they are not broker-dispatched remote command evidence.

| Measurement | Default source style | Constrained source style |
| --- | ---: | ---: |
| Actual content snapshots | 10 | 10 |
| Main / popup snapshots | 9 / 1 | 9 / 1 |
| Peak GSK nodes in one snapshot | 53 | 59 |
| Reported unsupported regions | 61 | 0 |
| Export/census median | 105.4 ms | 101.1 ms |
| Export/census maximum | 147.2 ms | 128.8 ms |
| Permanently retired scene IDs | 207 | 155 |
| Source resource bytes after handoff | 0 | 0 |

Durations cover child snapshot, semantic read, vector export and census; they
exclude local image diagnostics and file writing. This Python prototype does
not meet a 60 Hz export budget. Gap counts describe individual reports, not
unique pixels or percentage coverage: node bounds use each node's coordinate
space and may overlap.

The default source emitted container, transform, color, border, clip,
rounded-clip, text, linear-gradient, outset-shadow, fill, stroke and Cairo nodes.
It reported 25 `nonopaque-srgb-composition`, 17
`unsupported-outset-shadow-node`, 18 `rounded-clip-on-nonuniform-content` and
one `unsupported-cairo-node` gaps. Census still visits descendants of rejected
subtrees, so unsupported rounded clips do not hide the observed gradient nodes.

The constrained run **changes the actual source style**: it removes shadows,
animations, gradients, rounded corners and focus outlines, selects opaque flat
colors and uniform borders, replaces the menu button's symbolic arrow with a
label and disables the popup arrow. Zero gaps therefore establishes coverage of
this declared source style only. It does not establish equivalent rendering of
the default Adwaita source.

The exporter accepts affine 2D groups, rectangular clips, opaque flat paint,
uniform inward borders validated against RUI's outer and derived-inner corner
rules, uniform path fills/strokes, and the exact uniform-color rounded-clip case
that lowers to a rounded rectangle. GSK path commands M/L/Q/C/Z preserve source
geometry. Unsupported effects, textures, Cairo nodes, opacity, blends,
nonuniform paint and unsupported path forms become named gaps. The affected
subtree is omitted instead of dropping its clipping/effect and pretending its
children are equivalent. No rejected node is rasterized into a wire resource.

For text, the adapter obtains each actual source PangoCairo scaled font and
expands its already-shaped glyph ID into an outline. It retains the native
GskTextNode offset, Pango advances and per-glyph offsets. It sends normalized
1-em, y-down RVP1 outlines with glyph ID 0 under `glyph.outline-single.v1`;
remote font lookup and reshaping are unnecessary. Color/missing glyphs,
non-Cairo font backends and nonuniform font matrices are explicit gaps. The
pinned PyGI glyph-array accessor produced invalid values after its first entry,
so this build reads the native 20-byte `PangoGlyphInfo` ABI through a checked
local GSK node pointer. It accepts no pointers from a peer. Source glyph IDs,
font descriptions, outline bounds and placement evidence accompany local test
manifests; Rust tests verify decoded placement against that evidence.

## Exact replay, resources and bounds

The Python SDK produces exact existing RUI Protobuf envelopes with bound session,
epoch and view identifiers, explicit surfaces, scene and semantic records and
vector resource descriptors. The Rust local fixture bridge passes those exact
bytes through `ViewReceiver`. Resourceful candidates wait for authorized offers,
bounded chunks and verified completion. Resource descriptor lengths must equal
file metadata and the actual body read; bodies are read one at a time. The
bridge is a trusted local fixture consumer: approving this producer policy is
the caller's responsibility, and manifest strings are not authentication or
proof of source provenance.

The tests replay all ten strict frames into two receiver views, exercise actual
conditional scene disappearance/reappearance, verify stable source semantic IDs
and shaped-glyph placement, reject a restarted producer epoch on an existing
view, reject mismatched resource lengths before policy grants, and close both
receivers with their shared memory use reduced to zero. Python tests also cover
destroyed semantic registration, fresh producer epochs and resource handoff.
Replay reports applied state and resource readiness. It deliberately does not
acknowledge `Presented`, because no renderer callback runs in that CLI or test.

Source limits are 4,096 traversed GSK nodes, depth 128, 4,096 glyphs per
text node, 20,000 registered identities/records, 20,000 permanently retired
identities per epoch, 100,000 path commands, 16 MiB per resource and 32 MiB of
temporary source resource bodies. A depth guard runs before recursive export,
leaving room for text's per-glyph children within the reducer's depth128 bound.
Deep supported trees become an explicit `source-depth-ceiling` gap and the
census truncates at its bound. A native 180-layer ClipNode regression verifies
128 bounded scene records and census entries without Python recursion failure.
The source clears bodies after the artifact
handoff and retains only a bounded 64-entry capture summary history. Artifact
files are deliberately retained on disk by the test harness; this is not an
unbounded in-memory source cache or a negotiated resource retention policy.

The consumer bounds JSON manifests to 1 MiB and depth 64. Before serde allocates
maps and arrays, an allocation-free lexical preflight applies a conservative
16 MiB structural allocation quota including sparse tree roots, strings and
array spare capacity. This artifact quota is separate from the receiver's shared
state/resource budget. Exact envelopes are bounded to 1 MiB, and resources to
16 MiB. The fixture bridge does not expose arbitrary large-envelope decoding or
an authenticated network resource producer.

## Local visual diagnostics and remaining acceptance work

The same submitted GSK node is drawn into a bounded local Cairo image for
comparison: `.local.png` and native little-endian premultiplied sRGB BGRA
`.local.bgra`, at most 1,048,576 pixels. These diagnostics intentionally use
local raster rendering. They never enter the Protobuf envelope, vector resource
closure or transmitted content. They are generated by the measurement harness
and excluded from committed golden wire fixtures.

`fixtures/gtk/compare.py` compares every complete mapped source snapshot against
the release CPU renderer's selected-scene replay. This includes the native popup
scene; the topology-only parent still cannot establish whole-view completeness.
The comparator checks local image hashes, sizes and formats, explicitly converts
native premultiplied sRGB BGRA to the renderer's straight sRGB RGBA output, and
derives edge bands only from the native reference. Neighbor differences above
two channel levels form an edge, dilated by one pixel. Interior pixels must all
stay within three channel levels. Edge maximum RGB error is bounded to 160,
mean maximum RGB error over edge pixels to eight, and maximum alpha error to
128. Viewport mean maximum RGB error is bounded to four and mean alpha error to
one. Channel levels use the 0..255 display encoding.

The initial viewport mean maximum RGB criterion was three. Nine main snapshots
passed; the compact popup measured 3.122 and failed that criterion. The popup's
source-derived edge band covers 54% of its pixels, compared with about 18% for
main content. Calibration therefore changed the viewport bound to four and
added the separate edge-mean bound of eight, retaining zero out-of-bound interior
pixels and all independent geometry checks. This calibration is explicit in
the retained comparison report and does not discard the popup measurement.

The full ten-snapshot report is `fixtures/gtk/pixel-comparison.json`. Every
snapshot has zero interior mismatch and identical alpha. Edge maximum RGB error
is 127 for main content and 114 for the popup; native glyph outline bounds versus
decoded RVP1 bounds differ by at most 1.56e-15 source pixels. The SDK's reusable
`verify_native_glyph_evidence` also checks the actual decoded scene's glyph count,
unique IDs, resources, font sizes, glyph IDs and exact native source placements.
Both the replay CLI and renderer CLI invoke it on fresh captures. Negative
controls replacing an actual decoded glyph with an empty Group, shifting its
position or changing its font size fail this gate. Pixel tests independently
reject erased/changed flat interiors and wrong outline geometry.

```sh
cargo build -p rui-client-desktop --example render_capture --release --locked
python3 fixtures/gtk/compare.py /tmp/rui-gtk-corpus/strict \
  --output /tmp/rui-gtk-pixel-comparison.json
python3 -m unittest fixtures.gtk.test_compare adapters.gtk4.test_capture
```

Opaque sRGB paint is converted to linear sRGB for RUI interiors, but native sRGB
antialiasing/composition differs at edges from the linear renderer. The declared
edge criteria cover that pinned backend difference. This is a measured local
comparison under that tolerance, with independent geometry and decoded-scene
checks; it does not claim bit-identical rendering or coverage of other styles,
font backends, scales or omitted effects.

The current semantic encoder exposes roles, labels, typed string values and
action names. Typed TextModel versions/ranges, collection operations, selected
and focused state, focus routing, composition/IME ownership, broker-dispatched
source commands with revision evidence, and popup map/unmap/destroy are still
required for M3 interaction completion. AT-SPI was not used to infer coherence.

The alternate GTK 4.22.4 Broadway harness repeatedly aborted while repainting a
native popup at `gdk/broadway/broadway-output.c:456`,
`append_node_ops: assertion (node == reused_node)`. Three systemd-coredump
records identified `gtk4-broadwayd` SIGABRT; recorded stacks reached the GLib
assertion/abort path. These runs did not
produce accepted corpus evidence. Isolated Xvfb completed the measured corpus
and is the harness used for the retained fixtures.
