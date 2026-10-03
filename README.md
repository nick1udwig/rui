# RUI

RUI is an experimental remote UI state replication protocol. The host owns
application state and layout; clients retain authorized semantic and vector scene
state. The strict profile prohibits transmitted raster content.

The design is in [RFC.md](RFC.md). The original [conformance.yaml](conformance.yaml)
defines the intended full system. [Implementation acceptance](conformance/STATUS.md)
records the accepted milestones and their evidence.

RFC milestones M0–M5 and all 79 conformance cases are implemented and accepted
for the declared fixtures and isolated backend. The implementation includes a
bounded Rust core, pinned TLS/WebSocket transport, CPU vector rendering, two
GTK mirrors, a cooperating GTK producer with native wire composition, and
permissioned EIS session input. Exact traces replay through independent Rust
and Python implementations.

Acceptance covers the measured constrained GTK corpus and exported test session.
The [performance report](docs/performance.md) records missed provisional 60 Hz
budgets; arbitrary application/desktop support and RFC section 14 remain outside
this scope.

## Development

Install Rust 1.98.1, `protoc`, Python 3, `pkg-config`, and Cairo 1.18.4 development
files, then run:

```sh
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 conformance/check.py
python3 -m unittest adapters.sdk.test_producer
```

Transport tests require loopback socket access. The CPU renderer checks its
native Cairo version. CI uses [ci/build-cairo.sh](ci/build-cairo.sh) to build the
checksum-pinned source into an isolated prefix; Meson, Ninja and Pixman development
files are needed for that build. The GTK measurement harness has additional pinned
native dependencies described in [its coverage report](docs/gtk-coverage.md).

The Python conformance check independently decodes exact Protobuf fixtures and
compares Rust/Python logical state after accepted and rejected updates, including
all nine scene primitives. It needs no third-party Python packages. The workspace
lockfile pins Rust dependencies; toolkit handles do not belong in the wire schema.

## Components and examples

- [protocol-core](spec/core-profile.md): atomic replication, bounded publications,
  strict vector resources, receiver lifecycles, command admission and authority.
- [host-broker](host-broker/README.md): cooperating headless fixture, source evidence,
  independent observer projections and private-field redaction.
- [transport](transport/README.md): pinned TLS, explicit local pairing, bounded
  WebSocket messages, authorized glyph delivery and live-cache recovery.
- [client-desktop](spec/renderer-profile.md): pinned CPU reference rendering and
  guarded native GTK mirrors, local viewports, typing/preedit and PAM/PPM diagnostics.
- [producer SDK](adapters/sdk/README.md) and [GTK capture](docs/gtk-coverage.md):
  actual submitted content trees, stable source identities, guarded native actions
  and exact wire replay.
- [input and transcripts](spec/input-profile.md): bounded input interpretation,
  transient IME state and authorized diagnostic recording.

```sh
cargo run -p rui-host-broker --example controlled
cargo run -p rui-producer-sdk --bin replay_capture -- fixtures/gtk/golden
cargo run -p rui-client-desktop --example reference -- /tmp/rui-reference
cargo run --release -p rui-client-desktop --example render_capture -- \
  fixtures/gtk/golden/main-content-0001.json /tmp/rui-gtk.pam
```

Optional native mirror checks require the pinned GTK environment and an isolated
Xvfb executable; [the renderer profile](spec/renderer-profile.md)
lists the build and probe commands. The retained [two-mirror report](fixtures/render/native-corpus.json)
contains source/mirror hashes and scenario results without document text or credentials.

Unsupported required variants fail explicitly. The measured GTK scope is
cooperating content under declared styles; it does not establish arbitrary
application or whole-desktop coverage. See the [case audit](conformance/CASES.md)
for the accepted integration, visual, recovery and hardening evidence.
