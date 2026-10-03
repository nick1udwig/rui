# Measured GTK 4 cooperating capture

This foundation demonstrates an actual GTK content snapshot boundary, a
vector-only Protobuf publication accepted by the Rust receiver, and a local
native-versus-replay comparison of the full constrained corpus. It measures a
small ordinary-control corpus under GTK 4.22.4. It is not a generic GTK desktop
adapter, a persistent network producer or completed M3 interaction. A controlled
whole-view transaction slice is measured separately below.

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
These are source checks, not command authentication. These historical standalone
artifacts remain separate from the controlled whole-view transactions below. The private command fixture also publishes
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
is now 256 MiB, including a 4 MiB broker identity namespace allowance. Sparse node costs and retained String/Vec spare capacity are included
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

## Controlled whole-view source transaction

`whole_view.py` registers four actual native surfaces: the ordinary main content,
`Gtk.PopoverMenu`, a cooperating `Gtk.Popover` with Tooltip role, and a real
transient modal `Gtk.Window`. The tooltip is explicitly controlled source
content; it is not GTK's inferred internal tooltip implementation. Surface
placement and stacking are issued by this source for its isolated 720×600 canvas,
not sampled from a window manager. Native parent/transient relationships, mapping,
content sizes and modality are checked against registered objects.

A GTK-thread source actor starts one bounded transaction and reserves all fresh
candidate slots before requesting native snapshots. Each mapped registered
surface must execute its actual snapshot callback after the barrier begins. Its
same immutable node is appended to GTK and exported under that nonce. The actor
re-reads native topology, typed Semantic/Text/Collection payloads and clocks,
registered focus context and source revision before capture and final commit.
Changes discard the entire candidate union and restart; the producer never
combines unrelated latest frames. Strict CSS and GTK settings disable animation,
transitions and caret blinking, and the fixture has no uncontrolled visual timer.
This coherence argument applies to these cooperating static widgets, not arbitrary
animated native content. Unmapped surfaces receive explicit empty lifecycle
Groups; destroyed surfaces and their semantic ownership trees are removed.

The actual run published five complete whole-view snapshots. After retaining the
first candidate of nonce2, the fixture changed the heading and destroyed the
native tooltip. That candidate was discarded; nonce4 committed fresh main,
menu and modal captures at the new source boundary. Later native menu/modal
unmap, menu remap and menu destruction produce the expected mapped set. Menu
surface/semantic identities survive unmap/remap and retire at destruction.
One aggregate Session carries native registered focus, modality and the source
canvas. Unregistered native focus is reported as absent, never guessed.

`whole-interaction.json` binds 47 retained `.bin/.json/.rvp` files under
`whole-golden/` to their SHA-256 hashes. The Rust replay feeds all five exact
publications through one receiver, checks retired identities, disjoint ownership,
focus/modality, stable unrelated text clocks, exact vector/glyph closure and
absence of any stale interrupted token. The audit ties decoded Session focus to
the source boundary's surface/semantic mapping, validates native modality and
all decoded surface geometry. Altered nonce, membership, geometry, boundary focus,
modality, animation policy and explicit dependencies absent from source scenes
fail. No Presented acknowledgement is fabricated by replay.

The local reference independently draws each exact submitted node through native
GSK/Cairo and composes those diagnostics under the same declared source-owned
placement/stacking. This is a local diagnostic composition, not a desktop
screenshot; PNG/BGRA bytes stay outside envelopes, resources and retained vector
trace. All five decoded `render_view` outputs passed the existing criteria:
zero bad interior pixels, viewport mean RGB error ≤0.468, edge mean RGB error
≤3.609, and native glyph bounds error ≤1.56×10⁻¹⁵ source pixels. The comparison
checks 653 glyph instances across the five views. Existing omission/shift negative
controls remain required; full-source decoded geometry is checked before rendering.

The producer capture quota is 256 MiB, separate from native GTK application
memory and the receiver budget. Conservative charges cover fixed coordinator
metadata, retained per-surface frames, all candidates, boundary reads, exporter
scratch, encoding and the union resource file ownership. The measured peak is
235,982,499 bytes; cancellation/disposal drops candidates, frame slots and all
reservations to zero. Bounds are four surfaces, 4096 resources, 32 MiB union bodies,
1 MiB manifests/envelopes, at most four restarts and a three-second barrier deadline.
A native timer cancels a pending barrier even if no snapshot callback arrives.
The finite fixture directory has a separate 64 MiB/256-file quota covering vector
and local raster diagnostics. Unit regressions cover quota failure after actual
native node append, immediate cancellation, deadline without frames, descriptor
conflicts, failed handoff and synchronous restart inside candidate/request callbacks;
stale tickets cannot release new reservations. Consumer handoff is serialized and
cannot reenter a source mutation.

```sh
python3 fixtures/gtk/whole_check.py --xvfb /tmp/rui-xvfb/usr/bin/Xvfb \
  --output /tmp/rui-gtk-whole
cargo build --release -p rui-client-desktop --example render_capture
python3 fixtures/gtk/compare.py /tmp/rui-gtk-whole --whole-view \
  --output /tmp/rui-gtk-whole/whole-pixels.json
python3 fixtures/gtk/retain_whole.py /tmp/rui-gtk-whole
python3 -m unittest fixtures.gtk.test_transaction
cargo test -p rui-producer-sdk --test whole_trace
```

That retained five-frame whole-view run uses native cooperating fixture lifecycle
callbacks. The admitted-command whole-view connection is measured separately
below. Wire composition/IME ownership and the persistent/network producer path
remain required for M3. AT-SPI was not used to infer coherence. These are component measurements,
not a whole-milestone claim.

## Admitted commands and whole-view captures

`whole_native_source.py` connects the same private native dispatcher and one
`GtkCommandHost` broker/common ledger to the cooperating whole-view barrier.
The source explicitly binds widgets and its own Gio menu action to semantic
identities and owning surfaces. Core admission excludes unmapped surfaces and
modal-blocked owners, including popup descendants; the GTK thread rechecks
actual source identity, relevant clocks, modality, enablement and lease deadline
before any native callback. Semantic actions retain their target clocks across
unrelated scene/topology updates.

After native execution, mapping/layout notifications settle before a fresh
transaction reads its boundary. Each registered mapped surface contributes a
new actual snapshot; the identical immutable node is submitted locally and
lowered for export. The accepted union carries the exact admitted private request
ID, source generation and capture token. Native focus is read through explicitly
registered owners across main, popup and modal surfaces. Closing the modal
clears modality, preserves its unmapped identity, and publishes main focus;
closing/remapping the native menu preserves its surface and semantic IDs.

`whole-native-interaction.json` binds 63 exact vector files (eight full private
captures, eight authorized semantic-only snapshots and their resource closure).
Thirteen command scenarios establish seven actual dispatches, four pre-native
rejections and two exact retries with zero additional native calls. Effects
include modal Unicode editing with a scalar caret, a real modal-close callback,
GtkMenuButton map/unmap/remap, and the explicitly registered native Gio action.
An admitted button action deliberately interrupts its subsequent capture after
one candidate: a trusted race hook mutates semantics and destroys the tooltip.
The old nonce is aborted, no stale candidate token enters the accepted snapshot,
and the tooltip plus semantic subtree retire. This nested race is a diagnostic
negative control, not a second remote action. A redraw requested during native
snapshot submission is scheduled after that callback returns so a restarted
transaction cannot lose its fresh-frame request.

All eight exact `render_view` outputs passed the established GTK/Cairo-versus-CPU
criteria: zero bad interior pixels, viewport mean RGB error ≤0.575, edge mean RGB
error ≤3.776, and 1277 native glyph checks with bounds error ≤1.56×10⁻¹⁵ source
pixels. Each pixel report binds its envelope and local source-reference hashes.
Decoded scene/resource/geometry audits run for every capture before rendering.
Local BGRA composition and image-data drawing are counted outside the exporter;
the exporter probe rejects image-backed conversion, download, serialization and
raster fallback paths. Native GTK backend drawing remains outside that Python
probe boundary. Raster diagnostics are excluded from vector traces and caches.

The capture counter reserves an additional 8 MiB before retaining dispatcher
buffers and file inventories; measured capture peak is 244,372,754 bytes within
256 MiB, separately from GTK app memory and the Rust host budget. Native artifact
ownership retains only current/promised capture closures and prunes superseded
bodies and diagnostics, within 128 MiB/16384 files. Finite vector-trace and local
comparison directories each have their own 64 MiB quota. Quota regressions prove
consumed files release capacity for sustained replacement; host and replay
receiver disposal returns their reservations to zero. Barrier surface, candidate,
resource, restart and deadline ceilings are unchanged.

```sh
cargo build -p rui-producer-sdk --features gtk-fixture-hooks --bin gtk_command_check
python3 fixtures/gtk/native_check.py --xvfb /path/to/Xvfb \
  --output /tmp/rui-gtk-whole-native-empty --scenario whole-native
python3 fixtures/gtk/compare.py /tmp/rui-gtk-whole-native-empty/local-reference/whole-native \
  --whole-view --output /tmp/rui-gtk-whole-native-empty/pixels.json
python3 fixtures/gtk/retain_native.py /tmp/rui-gtk-whole-native-empty --name whole-native
cargo test -p rui-producer-sdk --test whole_native_trace
```

This is a bounded 30-second isolated cooperating-source component. Authorized
public consumers are separate instances of the same Rust reducer, receive only
semantics, and acknowledge semantic readiness without Presented. The full
private visual capture is verified and replayed separately; this run does not
establish network whole-view delivery, actual client Presented readiness or
wire composition. Those M3 integration gates remain open.

## Native editing context between frames

`native-context-interaction.json` retains a separate actual core-admitted source
probe. Focus notifications update the whole-view aggregate clock immediately,
including a native focus move away and back before another frame. In the
retained run the clock advances 7→9 while the focused entry identity returns to
the same value. A command admitted against clock7 fails at the final GTK guard
with zero additional dispatches, and an exact retry returns that retained failure.
The source resolves actual modal focus through registered widgets, including the
close button; an initial native focus assignment is not relabeled as entry focus.

Native GtkEditable content/selection notifications read the authoritative typed
model immediately. A backward selection advances its full record clock without
changing the content clock. A subsequent selection away and back within one
frame advances record clock2→4 while content clock remains1 and the final scalar
selection remains8→6. These intermediate native transitions cannot restore an
old editing context merely by restoring the final widget or selection. Event
reads acquire and release a conservative 16 MiB scratch reservation inside the
whole capture budget. Native text callbacks for a retired registry binding do
nothing.

If a native event read, reservation, or focus-clock increment fails, this source
incarnation permanently loses its native context authority. GObject callbacks
consume the error after setting that latch; later final dispatch and whole-view
boundary reads fail closed. Restoring the final value or available memory cannot
recover a missed transition. A fresh source incarnation is required. Injected
reservation/read failures and the focus-clock ceiling verify this behavior and
reservation cleanup.

The 48-file exact vector trace is SHA-bound, and Rust replay verifies all five
source captures, content versus record clocks, focus continuity and closure.
The `gtk-native-focus-context.v1` diagnostic profile binds the accepted Session
focus clock to the serialized source boundary; substituting that boundary clock
fails the audit before presentation. This profile is cooperating-source evidence,
not remote attestation. The probe uses semantic readiness, not Presented, and
makes no TextComposition or preedit claim; it is the source-context prerequisite
for the pending full visual/IME integration.

```sh
python3 fixtures/gtk/native_check.py --xvfb /path/to/Xvfb \
  --output /tmp/rui-native-context-empty --scenario native-context
python3 fixtures/gtk/retain_native.py /tmp/rui-native-context-empty --name native-context
cargo test -p rui-producer-sdk --test native_context_trace
```

## Broker continuity across native source exit

`GtkCommandHost::restart_source` is an explicitly trusted supervisor operation,
not a peer command. The controlled run in `restart-interaction.json` keeps the
same host PID, broker epoch, negotiated client/view and common command ledger
while an actual native source callback runs and its process receives SIGKILL
before a causal capture/reply. The result is UNKNOWN, with a local callback
receipt identifying the deliberate ambiguity. An exact retry while the source
is absent, after replacement, and after failed replacement starts returns the
same retained outcome and performs no new native dispatch.

Source exit revokes control, closes the full private receiver and its resource
cache (memory zero), and publishes a newer Session-only state: no app records,
focus, focus surface or cursor. Already connected observers and an observer
joining during absence consume that state. Replacement has a fresh source UUID
and private generation, fresh public app IDs, and a surviving broker Session ID.
The public revision advances 1→2 unavailable→3 replacement→4 native effect→5 idle
exit; source-local captures retain their original revisions and source identity.
All formerly exposed app IDs retire. A fresh sequence against an old target is
rejected; an admitted request carrying a delayed private generation fails at the
GTK guard with zero dispatches; an action against the new target reaches actual
GTK callbacks and its own generation-bound causal capture.

The finite evidence retains 41 exact `.bin`, `.json` and `.rvp` files, including
five authorized public Protobuf snapshots under one epoch and three full private
captures under two source incarnations. SHA-256 inventory in the report covers
every file. Rust replay verifies one continuous receiver's retirement, stable
Session, monotonic public/focus clocks, cleared focus and stripped semantic-only
resource closure, then checks each private capture's glyph geometry/vector audit
and the new action's exact source token. Pixel diagnostics remain local and are
excluded from this protocol trace. Consumers acknowledge semantic readiness and
never Presented.

Source identities are bounded to 4096 live records and 8192 issued public IDs,
including Session and all retired app/scene IDs. Four replacement attempts are
permitted; failed starts count. Namespace staging is atomic and charged with both
sparse map copies and the lifetime set inside the 4 MiB allowance. Dedicated
source artifact directories are emptied on replacement/drop, after current
capture ownership has moved; disposal permits at most 16384 regular files and
128 MiB. Actual failed starts preserve unavailable revision and existing views,
and final host drop returns all reservations to zero. The native process still
has a 30-second fixture watchdog, so these measurements establish a controlled
broker-preserving restart component rather than persistent network operation.
The private native source explicitly holds its Gtk.Application while the socket
is owned: destroying the last window cannot race the host's lifecycle
acknowledgement. The actual baseline gives GTK a main-loop opportunity to finish
last-window shutdown, verifies the source process remains alive, then consumes
retirement and rejects a late old-target command. Pipe closure/watchdog still
quit the held application.

```sh
cargo build -p rui-producer-sdk --features gtk-fixture-hooks --bin gtk_command_check
python3 fixtures/gtk/native_check.py --xvfb /path/to/Xvfb --output /tmp/rui-gtk-restart-empty --scenario restart
cargo test -p rui-producer-sdk --test restart_trace
```

The alternate GTK 4.22.4 Broadway harness repeatedly aborted while repainting a
native popup at `gdk/broadway/broadway-output.c:456`,
`append_node_ops: assertion (node == reused_node)`. Three systemd-coredump
records identified `gtk4-broadwayd` SIGABRT; recorded stacks reached the GLib
assertion/abort path. These runs did not
produce accepted corpus evidence. Isolated Xvfb completed the measured corpus
and is the harness used for the retained fixtures.
