# RUI

RUI is an experimental remote UI state replication protocol.
The host owns application state and layout; clients retain authorized semantic and vector scene state.
The strict profile prohibits transmitted raster content.

The design is in [RFC.md](RFC.md).
The original [conformance.yaml](conformance.yaml) defines the intended full system.
[Implementation acceptance](conformance/STATUS.md) records the accepted milestones and their evidence.

RFC milestones M0–M5 and all 79 conformance cases are implemented and accepted for the declared fixtures and isolated backend.
The implementation includes a bounded Rust core, pinned TLS/WebSocket transport, CPU vector rendering, two GTK mirrors, a cooperating GTK producer with native wire composition, and permissioned EIS session input.
Exact traces replay through independent Rust and Python implementations.

Acceptance covers the measured constrained GTK corpus and exported test session.
The [performance report](docs/performance.md) records missed provisional 60 Hz budgets; arbitrary application/desktop support and RFC section 14 remain outside this scope.

## Try the visual host/client demo

Run this from the repository root on a graphical Linux desktop:

```sh
cargo run --release -p rui-client-desktop --features gtk-demo --bin rui-demo --locked
```

Two windows open: **RUI Host** is the actual GTK application that owns its counter, text, list and menu; **RUI Client** renders the host's published state and sends editing commands back over TLS.
Click **Increment host counter**, edit the text field or select a list row in either window and watch the other view follow.
The client also offers control, zoom, reconnect and cache controls; request control again after reconnecting.
Close either window to stop the local demo.

The demo uses the development dependencies below plus GTK 4.22.4/X11 development files, Python PyGObject/PyCairo and OpenSSL.
It uses X11, including Xwayland on a Wayland desktop, and requires native Cairo 1.18.4.
See [the renderer profile](spec/renderer-profile.md) and [GTK coverage](docs/gtk-coverage.md) for the pinned native environment.
The launcher creates a private temporary directory containing a fresh TLS identity and controller credential and removes it on normal shutdown.
There are no scripted edits or automatic test timeouts in the manual demo.
The automated native checks remain separate; see [desktop controls](docs/desktop-controls.md).

### Host and client on separate machines

On tailnet machine A, serve the visible host on all IPv4 interfaces:

```sh
cargo run --release -p rui-client-desktop --features gtk-demo --bin rui-demo --locked -- --host-only --listen 0.0.0.0:8443 --identity-dir /tmp/rui-demo
```

The identity directory must not already exist.
Keep the host running and copy only `host.der` and `client.credential` to machine B.
For example, run these on B, replacing `USER` and `HOST_A` with A's SSH user and tailnet name or address:

```sh
scp USER@HOST_A:/tmp/rui-demo/host.der /tmp/rui-demo-host.der
scp USER@HOST_A:/tmp/rui-demo/client.credential /tmp/rui-demo-client.credential
```

Open the client on B, replacing `HOST_A` with A's tailnet name or address:

```sh
GDK_BACKEND=x11 GTK_IM_MODULE=simple cargo run --release -p rui-client-desktop --features desktop --bin rui-mirror --locked -- HOST_A:8443 rui-demo /tmp/rui-demo-host.der /tmp/rui-demo-client.credential 03030303030303030303030303030303 --control --scale=0.75
```

`rui-demo` in the client command is the generated certificate's TLS name; the network address is supplied separately.
The credential grants one demo controller permission to view and edit this application.
Keep private key files on A.
Close the host window or press Enter in A's terminal to stop the server; disconnecting B leaves A running for reconnection.
Each host launch creates a fresh identity and credential, so copy the new client files when restarting.
The listener defaults to an ephemeral loopback port when `--listen` is omitted; any explicit bind address is supported.

## Try the browser client

The TypeScript/Vite demo uses the same Rust receiver and a shared Rust session core through WASM. The host owns the visible GTK application; the browser renders its published vectors and can activate controls or submit guarded text edits.

With the native demo dependencies and Node installed, add the WASM target and matching CLI once, then launch both sides:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
python3 demos/web/run.py
```

Open the printed URL, choose the printed `client.credential` file, click **Connect**, then **Take control**. Click a button in the mirror, or click a text field and use **Apply to host** in its editor. Host-side edits also appear in the browser. For later runs, use `python3 demos/web/run.py --skip-build`.

To serve from tailnet machine A and open the browser on B:

```sh
python3 demos/web/run.py --skip-build --public-host HOST_A --listen 0.0.0.0:8443
```

Open `http://HOST_A:5173` on B and choose a copy of A's printed credential. B needs only a browser. The frontend binds all interfaces; its WebSocket proxy verifies the Rust host's certificate and preserves the Origin allowlist. HTTPS/WSS, separate-process commands, static output, tests and current capture limits are described in [the browser demo README](demos/web/README.md).

## Development

Install Rust 1.98.1, `protoc`, Python 3, `pkg-config`, and Cairo 1.18.4 development files, then run:

```sh
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 conformance/check.py
python3 -m unittest adapters.sdk.test_producer
```

Transport tests require loopback socket access.
The CPU renderer checks its native Cairo version.
CI uses [ci/build-cairo.sh](ci/build-cairo.sh) to build the checksum-pinned source into an isolated prefix; Meson, Ninja and Pixman development files are needed for that build.
The GTK measurement harness has additional pinned native dependencies described in [its coverage report](docs/gtk-coverage.md).

The manual demo's interaction and lifetime checks also run on an isolated display:

```sh
cargo build --release -p rui-client-desktop --features gtk-demo --bins --locked
python3 demos/check.py --xvfb /path/to/Xvfb
```

These check local and separate-process clients, rapid text edits, idle publication stability, and cleanup when either window closes.

The Python conformance check independently decodes exact Protobuf fixtures and compares Rust/Python logical state after accepted and rejected updates, including all nine scene primitives.
It needs no third-party Python packages.
The workspace lockfile pins Rust dependencies; toolkit handles do not belong in the wire schema.

## Components and examples

- [protocol-core](spec/core-profile.md): atomic replication, bounded publications, strict vector resources, receiver lifecycles, command admission and authority.
- [client-core](client-core/README.md): portable session, command recovery, hit testing and editing behavior shared with native/WASM clients.
- [client-wasm](client-wasm/README.md): Rust receiver/session bindings with generated TypeScript declarations, verified in Node and Chromium.
- [client-browser](client-browser/README.md): reusable browser transport, vector renderer and interaction API; [Vite demo](demos/web/README.md).
- [host-broker](host-broker/README.md): cooperating headless fixture, source evidence, independent observer projections and private-field redaction.
- [transport](transport/README.md): pinned TLS, explicit local pairing, bounded WebSocket messages, authorized glyph delivery and live-cache recovery.
- [client-desktop](spec/renderer-profile.md): pinned CPU reference rendering and guarded native GTK mirrors, local viewports, typing/preedit and PAM/PPM diagnostics.
- [producer SDK](adapters/sdk/README.md) and [GTK capture](docs/gtk-coverage.md): actual submitted content trees, stable source identities, guarded native actions and exact wire replay.
- [input and transcripts](spec/input-profile.md): bounded input interpretation, transient IME state and authorized diagnostic recording.

```sh
cargo run -p rui-host-broker --example controlled
cargo run -p rui-producer-sdk --bin replay_capture -- fixtures/gtk/golden
cargo run -p rui-client-desktop --example reference -- /tmp/rui-reference
cargo run --release -p rui-client-desktop --example render_capture -- fixtures/gtk/golden/main-content-0001.json /tmp/rui-gtk.pam
```

Optional native mirror checks require the pinned GTK environment and an isolated Xvfb executable; [the renderer profile](spec/renderer-profile.md) lists the build and probe commands.
The retained [two-mirror report](fixtures/render/native-corpus.json) contains source/mirror hashes and scenario results without document text or credentials.

Unsupported required variants fail explicitly.
The measured GTK scope is cooperating content under declared styles; it does not establish arbitrary application or whole-desktop coverage.
See the [case audit](conformance/CASES.md) for the accepted integration, visual, recovery and hardening evidence.
