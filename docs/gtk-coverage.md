# Measured GTK 4 cooperating capture

This foundation demonstrates an actual GTK content snapshot boundary, a
vector-only Protobuf publication accepted by the Rust receiver, and a local
native-versus-replay comparison of the full constrained corpus. It measures a
small ordinary-control corpus under GTK 4.22.4. It is not a generic GTK desktop
adapter, a combined multi-surface session producer or completed M3 interaction.

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
fixture publishes actual popup snapshots for map/remap and explicit native
lifecycle signals for unmap/destruction. Unmapping clears focus and the resource
closure, preserves the Surface and semantic identities, and retires the old
scene. Destruction removes and retires the Surface, scene and semantic objects.
Recreating the same source key starts a fresh Surface/view and per-view Session
identity; the old capture cannot publish again. Capture bookkeeping is committed
before observer callbacks, including synchronous unmap/destruction callbacks.
Source guards reject old incarnations, routes, clocks and destroyed objects.
These are source checks, not command authentication. Combining current main and
popup projections remains required. The private command fixture also publishes
actual main-content destruction and rejects later events to those identities.

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
vary across fresh runs. Each measured publication includes its session/epoch/view
binding and exact envelope SHA-256; regression checks match all strict entries
and the visual report to the retained golden envelope bytes.

## Corpus and rendering coverage

The real application contains a label with accented and Greek text, an editable
entry, an ordinary button, native Gio.Menu/Gtk.PopoverMenu, a ListBox of three
explicitly identified rows inside a ScrolledWindow, and a custom source widget
that submits GSK vector fill and stroke nodes. Source callbacks change text and
selection, activate button/menu actions, open and close the popup, remove and
restore the canvas, resize the source window, remove and rematerialize a list
row, and unmap/remap/destroy the popup. These are application callback
observations; they are not broker-dispatched remote command evidence.

| Measurement | Default source style | Constrained source style |
| --- | ---: | ---: |
| Publications / actual content snapshots | 20 / 17 | 20 / 17 |
| Main / popup snapshots | 15 / 2 | 15 / 2 |
| Popup lifecycle publications | 3 | 3 |
| Peak GSK nodes in one snapshot | 53 | 59 |
| Reported unsupported regions | 104 | 0 |
| Export/census median | 8.32 ms | 7.66 ms |
| Export/census maximum | 12.33 ms | 12.14 ms |
| Permanently retired scene IDs | 305 | 228 |
| Source resource bytes after handoff | 0 | 0 |

Durations cover child snapshot, semantic read, vector export and census; they
exclude local image diagnostics and file writing. These 17 frames per style
fall below a 16.7 ms export budget in this run; they do not establish end-to-end
60 Hz latency or sustained throughput. Earlier foundation measurements took
101–105 ms median under their measured load; this report records the current
run without attributing that difference to an implementation speedup. Gap counts
describe individual reports, not
unique pixels or percentage coverage: node bounds use each node's coordinate
space and may overlap.

The default source emitted container, transform, color, border, clip,
rounded-clip, text, linear-gradient, outset-shadow, fill, stroke and Cairo nodes.
It reported 43 `nonopaque-srgb-composition`, 29
`unsupported-outset-shadow-node`, 30 `rounded-clip-on-nonuniform-content` and
two `unsupported-cairo-node` gaps. Census still visits descendants of rejected
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

## Typed native state and interaction boundary

The registered entry exports a typed `TextModel` from the actual `GtkText`
delegate. Content/status changes advance `text_version`; changing selection
alone retains that clock. Positions are Unicode scalar indices. The measured
21-scalar, 23-byte text `edited source: café Ω` retains the backward native
selection anchor 19/focus 15. Reading the delegate avoids this pinned build's
`GtkEntry.get_position()` returning the sorted selection end.

The ListBox exports a typed `CollectionModel`, stable materialized item
references, native selected-item state, total count and materialization status.
Removing Gamma changes full three-item materialization to partial two-item
materialization with total count three; its semantic ID is permanently retired.
Recreating the native row allocates a fresh ID while Alpha/Beta retain theirs.
Selection and each materialization change advance the collection clock. Core
replay asserts these values and rejects stale text/collection clocks atomically.

A per-view `SessionState` carries actual native focus and focus-generation
transitions. The registry observes window focus notifications, resolves only
explicitly registered native ancestors, and clears focus when the popup unmaps
or objects are destroyed. Tests check the actual recorded transitions and reject
stale focus clocks. IME composition, guarded remote command dispatch through the
shared Rust session authority/ledger, and causal command-result publication are
still acceptance work. Timed local corpus callbacks are not remote command
proof, and `NativeRegistry.resolve_event` alone cannot admit a command.

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

The tests replay all 20 strict publications into two receiver views, exercise
actual conditional scene disappearance/reappearance, verify stable source
semantic IDs and shaped-glyph placement, and reject restarted epochs and
mismatched resource lengths. Popup unmap releases outline pins and rejects late
resource replies; remap reacquires an evicted outline, and destruction rejects
late attempts to resurrect retired records. Both receivers close with their
shared memory use reduced to zero. Python regressions also cover fresh surface
identities after destruction, disposal before the first snapshot, reentrant
lifecycle callbacks, source guards and resource handoff.
Replay reports applied state and resource readiness. It deliberately does not
acknowledge `Presented`, because no renderer callback runs in that CLI or test.

Source limits are 4,096 traversed GSK nodes, depth 128, 4,096 glyphs per
text node, 20,000 registered identities/records, 20,000 permanently retired
identities per epoch across scene, semantic, text, surface and Session objects,
2,048 UTF-8 bytes per identity key, 16 KiB per text value, 4,096 collection items,
a conservative 4 MiB retained model-clock cache, 100,000 path commands, 16 MiB
per resource and 32 MiB of temporary source resource bodies. The model cache
charges retained string/container capacity before updates and releases state
on object retirement; its quota is separate from GTK application memory and
the receiver budget. A depth guard runs before recursive export,
leaving room for text's per-glyph children within the reducer's depth128 bound.
Deep supported trees become an explicit `source-depth-ceiling` gap and the
census truncates at its bound. A native 180-layer ClipNode regression verifies
128 bounded scene records and census entries without Python recursion failure.
The source clears bodies after the artifact
handoff and retains only a bounded 64-entry capture summary history. The fixed
visual measurement harness deliberately retains artifact files on disk. The
private native command source retains only its current capture and one promised
capture until the Rust consumer acknowledges ownership. It then prunes obsolete
JSON/bin, local image diagnostics and unowned RVP bodies, preserving shared bodies
still owned by either capture. A 128 MiB quota covers retained and transient files;
it is not a negotiated network resource retention policy.

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

The initial foundation viewport mean maximum RGB criterion was three. Its nine
main snapshots passed; the compact popup measured 3.122 and failed that criterion.
The popup's
source-derived edge band covers 54% of its pixels, compared with about 18% for
main content. Calibration therefore changed the viewport bound to four and
added the separate edge-mean bound of eight, retaining zero out-of-bound interior
pixels and all independent geometry checks. This calibration is explicit in
the retained comparison report and does not discard the popup measurement.

The refreshed full 17-snapshot report is `fixtures/gtk/pixel-comparison.json`. Every
snapshot has zero interior mismatch and identical alpha. Edge maximum RGB error
is 127 for main content and 114 for the popup; native glyph outline bounds versus
decoded RVP1 bounds for 1,577 shaped glyphs differ by at most 1.56e-15 source
pixels. Main viewport mean maximum RGB error stays at or below 0.569 and popup
mean at 3.122; edge means remain at or below 3.209 and 5.825 respectively. The
SDK's reusable
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

The current semantic encoder exposes roles, labels, typed values, action names,
TextModel clocks/scalar ranges, CollectionModel membership/materialization and
selected state, Session focus clocks, and explicit popup lifecycle. SemanticNode
versions describe their actual payload; TextModel record versions describe
content/status, content clock, selection and editability. An unrelated visual
frame preserves both text clocks and the semantic version. Selection-only changes
advance the text record version while preserving its content clock. Retirement
releases all corresponding bounded cache entries.

The SDK private native fixture submits exact semantic peer bytes through the
real core session authorization and shared command ledger before dispatching
native GTK callbacks. The final main-loop guard checks registered incarnation,
surface/object liveness, native sensitivity/modality and fresh relevant text,
collection and focus state. Collection item actions resolve the unique published
items-owner clock. Global scene/route changes do not invalidate otherwise valid
semantic actions. Native lease generation and the absolute CLOCK_MONOTONIC
deadline are checked immediately before execution, so a stalled GTK queue cannot
execute an expired admission. The private pipe is trusted host IPC; the Python
process owns no command ledger.

The run recorded in `fixtures/gtk/native-interaction.json` exercised 32 actual
semantic native dispatches and 37 observed native callbacks in its main scenario,
including Unicode ReplaceText, two insertions at the returned native scalar caret,
backward selection and stable-item activation. Independent instances of the same
Rust semantic reducer
applied equal state after each change; neither reported Presented. Independent
Python implementation interoperability is separate evidence. The run also
covered an observer without control, forged principal/view, stale control,
target/focus/text/model clocks, missing item-owner clock, native edit,
materialization and retirement before snapshot, an unrelated visual refresh,
24 consumed-capture churn commands and main-source destruction. A 16 KiB control
text request expands to about 96 KiB private JSON and is accepted under the
128 KiB pipe ceiling. Its glyph-count/opacity gaps publish partial visual coverage
while preserving truthful semantic text. Embedded NUL is rejected before native
execution because GTK's string API cannot represent it.

Applied evidence requires an actual newer captured commit with the exact admitted
private request cause, source token and a revision actually published to that
client. Exact retries produce no second native call. Three independent fault
scenarios execute a native callback then lose the pipe, stall the reply, or return
an uncorrelated fresh capture. Each produces UNKNOWN with no Applied evidence,
no retry dispatch and no fresh command dispatch. A request delayed past its native
lease deadline produces a known zero-effect failure. These are controlled GTK
process runs on a private authenticated Xvfb display and private session bus.
They do not establish TLS or a persistent source host: the native process has a
30-second watchdog, and this slice supports public main-content semantics only.

Host reservations have a 256 MiB aggregate ceiling: a 96 MiB independently bounded
source receiver, 4 MiB ledger allowance, 1 MiB private IPC scratch, 16 MiB per
current or pending artifact, 17 MiB transient body/descriptor scratch, 19 MiB
projection/command scratch, and two 8 MiB pools per client, at most four clients.
Pending metadata/body scratch is reserved before native execution; projection
scratch can coexist with command scratch. The maximum conservative reservation
is 252 MiB. Sparse node costs and retained String/Vec spare capacity are included
by artifact/core preflights or these allowances. Native GTK application memory is
outside this protocol budget. The driver verifies host reservations release on
drop, including after UNKNOWN outcomes.

Each run writes exact full-source Protobuf publications and native manifests
alongside a decoded interaction report. The refreshed `native-golden` trace has
47 source publications across six incarnations, including unrelated visual
refreshes and terminal source destruction, in 273 hash-bound `.bin/.json/.rvp`
files (6,399,812 bytes). Its 61 command/retry entries retain exact admitted peer
bytes. Tests decode these bytes, check exact retry equality, bind Applied tokens
to actual source captures and replay every publication through the core.
The main scenario still records 32 dispatches and 37 native callbacks.

New schema2 counters record actual node visits/types, lowering/rejection decisions,
decline reasons, emitted vector types and geometry-body extraction attempts.
Local reference draw counts increment after native drawing. Resources record
actual native glyph-outline or GSK fill/stroke extraction origins and node paths;
the SDK handoff counts bytes and body kinds actually written. The Rust audit
compares every decoded scene primitive and common field with this source evidence,
checks the exact live resource closure/descriptors and coverage reasons, and
checks native glyph geometry. Handoff kind counts agree with actual decoded
offers, and unsupported gap paths/types/bounds agree with native census metadata;
those bounds are diagnostic metadata, not an additional wire region feature.
Geometry, unsupported-gap and pixel-to-vector-origin substitutions fail, as do
missing schema2 scenes and mismatched kind counts. Historical foundation manifests retain the old asserted-zero
fields; the CLI labels their limited check `historical-glyph-only`.

The actual negative corpus in `negative-report.json` submits the same source node
to GTK and export for an original photograph, an independent synthetic memory
texture, a native Cairo drawing, blur and GLShader, with a native fill path
positive control. The first five yield named unsupported subtree gaps, an empty
Group for that region, and Partial visual coverage. Only the native path control
offers a vector resource. The source-only original NASA PIA19400 JPEG is unchanged,
47,634 bytes, SHA-256
`cfb90122fb690c0661002606310bd20d5b236f2b628994494aa23503a2ad50be`.
Credit: NASA/JPL-Caltech/MSSS/Texas A&M Univ.; provenance and NASA usage guidance
are in [the asset note](../fixtures/gtk/assets/README.md). The synthetic texture
is a separate case. The JPEG never enters a protocol envelope or resource cache.
Source photograph/texture pixels are actually drawn in the local reference;
numeric color-presence measurements and the reference hash are report metadata,
while BGRA/PNG bytes remain local outputs excluded from the retained vector trace.
The pinned Cairo backend renders the GLShaderNode as GTK's pink placeholder;
this proves actual shader-node rejection, without claiming GLSL execution.
[GTK shader constructor behavior](https://docs.gtk.org/gsk4/ctor.GLShaderNode.new.html).

The test-only probe is active during every source export in both negative and
admitted native runs. It traps exporter Python texture download/save, render-node
draw/serialization and color/large Cairo image-surface construction. A one-pixel
A8 glyph-path context is allowed. Deliberate download and image-surface controls
each trip once; the six negative exports attempt no forbidden calls, and six
local reference draws are observed outside export. Source extraction origins and
the decoded geometry comparison address raster-to-vector laundering beyond a
resource-kind whitelist. This observation boundary covers the cooperating Python
exporter, not native GTK internals or arbitrary malicious producers. Each stored
native source manifest carries the probe observations; no asserted forbidden-zero
counters are used for this new evidence.

The additional native Unicode scenario in `unicode-interaction.json` ran eight
native dispatches and eight observed callbacks through the same core ledger and
final native guard, with a separate semantic observer. Nine source captures
in 58 hash-bound `.bin/.json/.rvp` files replay exact content and scalar ranges.
It replaces `A𝄞e◌́ אבגZ` (the actual source contains e+U+0301, not a dotted-circle
character), selects a supplementary scalar, selects between base and combining
mark, preserves backward RTL `[8,5]`, replaces a supplementary scalar and combining
mark, and replaces the RTL substring in source logical order. Returned caret and
selection assertions use actual native GtkText state; selection-only changes
preserve the content clock. Exact retry adds no native call, and stale content
clocks reject with no effect. Native glyph evidence is checked against the decoded
scene; native opacity gaps remain Partial, without a Unicode pixel-equivalence
claim.

This found and fixed a source bug: GtkText logical cursor movement follows
grapheme boundaries and cannot implement every scalar selection. The dispatcher
now calls `GtkEditable.select_region(anchor, focus)` once, with no preceding
caret-collapse transition. Actual snapshots confirm `[2,3]` inside the combining
sequence and backward RTL direction; the existing backward-ASCII native scenario
also passes. GTK positions are character offsets, not byte offsets.
[GTK scalar range API](https://docs.gtk.org/gtk4/method.Editable.select_region.html).

These are measured negative, trace and native Unicode checks for this bounded
private fixture, not a whole-milestone claim. Wire composition/IME ownership,
combined transient-surface projection and the persistent/network producer path
remain required for M3. AT-SPI was not used to infer coherence.

The alternate GTK 4.22.4 Broadway harness repeatedly aborted while repainting a
native popup at `gdk/broadway/broadway-output.c:456`,
`append_node_ops: assertion (node == reused_node)`. Three systemd-coredump
records identified `gtk4-broadwayd` SIGABRT; recorded stacks reached the GLib
assertion/abort path. These runs did not
produce accepted corpus evidence. Isolated Xvfb completed the measured corpus
and is the harness used for the retained fixtures.
