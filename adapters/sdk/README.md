# Cooperating producer SDK

`producer.py` supplies explicit identities, synchronous semantic capture, bounded
vector resources and an exact RUI Protobuf snapshot encoder. The GTK adapter in
`../gtk4/capture.py` supplies the actual source snapshot boundary. This package
does not authenticate a producer, discover authorization, fetch assets or route
network commands.

The Rust `CaptureArtifact` bridge consumes local fixture manifests and their
exact `.bin` envelopes and `.rvp` resources through `ViewReceiver`. Calling
`apply_trusted_fixture` means the caller approved that local producer's resource
policy; reading a JSON manifest is not evidence of trust or actual GTK
provenance. The bridge enables the explicit `glyph.outline-single.v1` profile.
It never acknowledges presentation. A renderer must complete rendering before
calling the receiver's presentation method.

Fixture renderers must call `verify_native_glyph_evidence` after applying the
envelope. It compares the native manifest evidence to the actual decoded scene,
including glyph count, identities, resources, sizes and exact source placements.
This diagnostic check prevents omitted or shifted glyphs from passing an edge
pixel tolerance; it does not authenticate manifest provenance.

Fresh captures also expose `verify_vector_audit`, which checks every decoded
scene's primitive/geometry and common fields against the full source manifest,
the exact resource closure and geometry-only descriptors, source extraction
origins/census paths and named coverage gaps. Handoff kind counts must equal
decoded offers; gap type/path/bounds must match the native census metadata.
It includes the glyph check.
`GtkCommandHost` requires this audit before admitting a source capture. Schema2
contains observed exporter operation counts, not asserted zero fallback fields.
The replay CLI labels early foundation manifests without full scene evidence
`historical-glyph-only`; this limited path is separate from the fresh source gate.

```sh
python3 -m unittest adapters.sdk.test_producer
cargo test -p rui-producer-sdk --locked
cargo run -p rui-producer-sdk --bin replay_capture -- fixtures/gtk/golden
```

The capture manifest has a 1 MiB byte ceiling, depth 64 and a conservative 16 MiB
structural allocation preflight before JSON deserialization. This local artifact
quota is separate from the receiver's shared state/resource budget. Exact
envelopes have a 1 MiB ceiling; each vector resource has a 16 MiB ceiling and is
read separately. Declared resource lengths must equal both file metadata and
the actual body read. These are fixed fixture limits, not a negotiated network
transfer API.

See [the measured GTK coverage report](../../docs/gtk-coverage.md) for the source
scope, pinned environment, local image diagnostics, unsupported rendering and
remaining interaction work.

`gtk_host::GtkCommandHost` is a bounded cooperating-source command fixture.
Trusted host code supplies the source process and already verified authorization,
plus trusted elapsed monotonic milliseconds to the session methods. Every exact
peer command enters the core's one `CommandLedger::submit_controlled_with_session`.
Only its admitted callback can send a native command over the inherited private
Unix socket. Source incarnation, surface/object liveness, native enablement,
relevant clocks, popup modality and the admitted lease's absolute Linux monotonic
deadline are rechecked on the GTK main-loop thread immediately before execution.
The socket is not a peer endpoint and the Python process owns no command ledger.

Only public semantic projections are supported. Scenes, bindings, visual links,
cursor and resource closure are removed; each projection has an actual private
semantic consumer. That consumer acknowledges Applied and semantic readiness,
never Presented. Joining observers do not reset existing readiness; a client
whose negotiated pool cannot hold a projection closes independently. Source
authority always comes from the full private source receiver.

Applied requires a newer valid same-source capture carrying the exact private
request ID and authority marker. Its source token and actual published revision
become the retained core result. Pipe loss/deadline after possible native
execution, or a fresh capture without that cause, becomes UNKNOWN and revokes
control. Exact retries return the retained result without another native call.
The process has a 30-second watchdog: this is a bounded fixture, not a persistent
network host. Raw input, wire composition/IME and resize are unsupported here.

```sh
cargo build -p rui-producer-sdk --features gtk-fixture-hooks --bin gtk_command_check --locked
python3 fixtures/gtk/native_check.py --xvfb /path/to/Xvfb --output /tmp/rui-gtk-native-empty
python3 -m unittest adapters.gtk4.test_native adapters.sdk.test_producer
```

The integration driver uses a private authenticated Xvfb display and a private
`dbus-run-session`, with no connection to the personal compositor/session bus.
`gtk-fixture-hooks` enables explicit trusted race/fault controls, never decoded
from peer command bytes. The output contains exact full-source snapshots,
manifests and a decoded interaction report. These finite test trace copies are
separate from the native source's bounded current/pending artifact ownership.

The separate native Unicode scenario uses the same core ledger and final source
guard for supplementary, combining and RTL content, with a separate semantic
observer. `SetSelection` makes one native `GtkEditable.select_region` call;
grapheme cursor movement cannot implement arbitrary scalar offsets, and an
intermediate caret collapse can lose the selection before capture. Actual source
snapshots verify a supplementary scalar, a selection inside a combining sequence,
backward RTL direction, scalar replacements/carets and unchanged content clocks
on selection changes. Exact retry has no native effect; stale text is rejected.

```sh
python3 fixtures/gtk/native_check.py --xvfb /path/to/Xvfb --output /tmp/rui-gtk-unicode-empty --scenario unicode
```

`unicode-interaction.json` and `unicode-golden` retain nine exact source captures
and ten command/retry entries from eight real native dispatches/callbacks. These
source state/geometry checks do not establish wire IME or cross-backend Unicode
pixel acceptance; native opacity gaps are preserved as Partial coverage.

The no-fallback negative corpus submits actual photograph/texture, Cairo, blur
and GLShader nodes, plus a native path control. It records Partial coverage with
specific unsupported reasons. The original NASA JPEG has an exact hash and
credit in [assets/README.md](../../fixtures/gtk/assets/README.md) and never becomes
a resource. The shader uses the pinned native Cairo pink placeholder; this
establishes source-node rejection, not successful GLSL execution. Pixel images
are local diagnostics, excluded from the retained `.bin/.json/.rvp` trace.

```sh
python3 fixtures/gtk/negative_check.py --xvfb /path/to/Xvfb --output /tmp/rui-gtk-negatives-empty
python3 -m unittest fixtures.gtk.test_negatives
cargo run -p rui-producer-sdk --bin replay_capture -- fixtures/gtk/negative-golden
```

The test-only `FallbackProbe` traps exporter Python calls to texture download or
image save, node draw/serialization, and color/large Cairo image surfaces. A
one-pixel A8 context for native glyph path extraction is allowed. Deliberate
download/image-surface calls prove that the traps fire. Native GTK internals and
local drawing outside source export are excluded. The complete consumed native
trace is refreshed with those probes active, including unrelated refreshes and
source destruction; exact peer commands/retries and all retained publications
are decoded and hash-bound in tests. This is bounded cooperating fixture
evidence, not provenance attestation for arbitrary producers or full M3 closure.
