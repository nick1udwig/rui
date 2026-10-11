# RUI

RUI is an experimental remote UI state replication protocol.
The host owns application state and layout; clients retain authorized semantic and vector scene state.
The strict profile prohibits transmitted raster content.

The design is in [RFC.md](RFC.md).
The original [conformance.yaml](conformance.yaml) defines the intended full system.
[Implementation acceptance](conformance/STATUS.md) records the accepted milestones and their evidence.
The [implementation plans](docs/implementation-plans.md#validation-commands-and-completion-records) record the shipped work on performance, application coverage, reusable clients and semantic presentation, with remaining acceptance limits.

RFC milestones M0–M5 and all 79 conformance cases are implemented and accepted for the declared fixtures and isolated backend.
The implementation includes a bounded Rust core, pinned TLS/WebSocket transport, CPU vector rendering, two GTK mirrors, a cooperating GTK producer with native wire composition, and permissioned EIS session input.
The current browser library adds a vector mirror and a responsive semantic UI for two cooperating applications, with explicit control, guarded edits, dirty notifications and incremental publications.
Exact traces replay through independent Rust and Python implementations.

Acceptance covers the measured constrained GTK corpus and exported test session.
The [performance report](docs/performance.md) records missed provisional 60 Hz budgets; arbitrary application/desktop support, independent application views and concurrent writers remain outside this scope.

## Best demos to try

Start with the **semantic editor** to see the alternative web presentation, then the **browser vector mirror** to compare it with the source layout. The **native host/client pair** is the quickest demo if you already have the Rust/GTK environment and want to avoid browser setup. Run the browser launchers one at a time; both use port 5173.

### Semantic editor: host state, responsive web layout

Use Rust 1.98.1, Node 26.8.1, `protoc`, `pkg-config`, GTK 4.22.4/X11, Cairo 1.18.4, Python with PyGObject/PyCairo, and OpenSSL. Xwayland works on a Wayland desktop. From the repository root, install the matching WASM tooling once:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
python3 demos/web/run.py --presentation semantic --application editor
```

The launcher builds the host, WASM and browser library, installs locked npm dependencies, and opens the real GTK document editor. Open the printed browser URL, choose the printed `client.credential` file, click **Connect**, then **Take control**.

1. Under **Tasks** or **Field notes**, choose **Select source item** and watch the GTK selection and fields follow.
2. On **Body**, choose **Edit draft**, enter `café Ω 𝄞 é שלום`, and choose **Apply draft**. The native field should show the committed string. Local typing remains a draft until you apply it.
3. Use **Activate** under **Save document**, toggle **Reviewed**, and activate **Document details** and **Close details**. Source status, Boolean state and the modal relationship should update in both views.
4. Resize the browser to a narrow window. Its semantic layout reflows while the source window keeps its own layout. The private field remains redacted in the browser.
5. Start another draft, change the same source field in GTK, and apply the old draft. It should report a conflict and preserve the local draft. Disconnect/reconnect, then explicitly take control again.

Close the host or press Ctrl+C in the launcher terminal to stop. For later runs, add `--skip-build`; the selected demo must already have its dependencies installed. Use `--application corpus` to try the original counter/text/list application. See [semantic presentation](demos/semantic/README.md) for the interaction and accessibility contract.

### Native host and vector client

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

#### Host and client on separate machines

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

### Browser vector mirror

The TypeScript/Vite demo uses the same Rust receiver and a shared Rust session core through WASM. The host owns the visible GTK application; the browser renders its published vectors and can activate controls or submit guarded text edits.

With the dependencies and WASM tooling above, launch both sides:

```sh
python3 demos/web/run.py
```

Open the printed URL, choose the printed `client.credential` file, click **Connect**, then **Take control**. Click a button in the mirror, or click a text field and use **Apply to host** in its editor. Host-side edits also appear in the browser. For later runs, use `python3 demos/web/run.py --skip-build`.

This is the source-shaped vector presentation, rendered from transmitted geometry and outline glyphs. Use `--application editor` to compare the document editor with the semantic demo. Switch away from the tab and return: the client should resume displaying state and require **Take control** again.

To serve from tailnet machine A and open the browser on B:

```sh
python3 demos/web/run.py --skip-build --public-host HOST_A --listen 0.0.0.0:8443
```

Open `http://HOST_A:5173` on B and choose a copy of A's printed credential. B needs only a browser. The frontend binds all interfaces; its WebSocket proxy verifies the Rust host's certificate and preserves the Origin allowlist. HTTPS/WSS, separate-process commands, static output, tests and current capture limits are described in [the browser demo README](demos/web/README.md).

## Best live tests

These tests start actual owned GTK applications and clients on private authenticated Xvfb displays and a private session bus. They exercise the live source and transport; the portable browser suites linked below are separate. Run them sequentially, with the manual launchers stopped so port 5173 is free. Do not rebuild browser/WASM assets while a live check is running.

Use the pinned native/browser dependencies above plus Xvfb, `xauth`, `dbus-run-session`, libX11 and libXtst (XTest). Prepare the artifacts from the repository root:

```sh
cargo build --release -p rui-client-desktop --features gtk-demo --bins --locked
python3 client-wasm/build.py
npm --prefix client-browser ci
npm --prefix client-browser run build
npm --prefix demos/web ci
npm --prefix demos/web run build
npm --prefix client-browser exec -- playwright install chromium
export RUI_XVFB="$(command -v Xvfb)"
```

If Xvfb is outside PATH, set `RUI_XVFB` to its executable's absolute path. To use an existing full Chromium installation instead of the Playwright download, export `RUI_WASM_CHROMIUM=/path/to/chromium`. Playwright's browser needs its system libraries; [browser CI](docs/browser-ci.md) documents the tested environment. Replace `NEW` below with a fresh run suffix: the presentation and native capture checks require output directories that do not already contain a run.

### 1. Semantic controller + vector observer: strongest feature check

```sh
RUI_RESOURCE_RECOVERY=1 python3 demos/semantic/presentation_check.py --application editor --xvfb "$RUI_XVFB" --output /tmp/rui-presentation-editor-NEW
RUI_RESOURCE_RECOVERY=1 python3 demos/semantic/presentation_check.py --application corpus --xvfb "$RUI_XVFB" --output /tmp/rui-presentation-corpus-NEW
```

Two separately approved identities connect to one host, swap the single control lease, and activate buttons, apply Unicode text and select stable items from both presentations. The checks cover stale drafts, delayed results, resource-cache loss, reconnect, source restart and reduced permissions; the editor adds partial collections, a modal and control replacement. Exact authorized packets replay through the independent Python oracle. The semantic client receives no Scene/Binding records or glyph resources and sends no visual presentation acknowledgments. Reports and packet evidence are in each output directory's `semanticOnly.json` and `mirror.json`.

### 2. Production browser mirror: HTTPS and real tab visibility

```sh
RUI_BROWSER_HEADED=1 RUI_BROWSER_HTTPS=1 RUI_BROWSER_BUILD=1 python3 demos/web/check.py --xvfb "$RUI_XVFB" --timeout 120
```

This serves the built Vite output over HTTPS with a certificate-verifying WebSocket proxy. It checks observer restrictions, browser and native edits, Unicode, reconnect, actual hidden-tab control release and shutdown cleanup. Headed Chromium runs on the private test display. The screenshot and report are `client-browser/test-output/demo.png` and `timings.json`; this correctness check does not establish a latency budget.

### 3. Installed package: production consumer outside the checkout

```sh
RUI_BROWSER_HEADED=1 python3 client-browser/tests/package-live.py --xvfb "$RUI_XVFB"
```

This packs the private runtime, installs it in a temporary consumer outside the repository, type-checks and builds it, then connects that installed code to the real host. It tests production HTTPS under `/apps/rui/`, default and replacement adapters, guarded edits, reconnect, real browser back/forward cache restoration and disposal. The report is `client-browser/test-output/package-live.json`. The first run needs npm registry access; a warm dependency cache can use `RUI_PACKAGE_OFFLINE=1`.

For repeated ownership and recovery checks, run the same command with `RUI_LIFECYCLE=1`. It checks 25 connect/disconnect/dispose cycles, resource recovery, delayed acknowledgments and hidden-tab behavior, and writes `client-browser/test-output/lifecycle.json`.

### 4. Native mirror and IME: actual source pixels and shared actor

```sh
cargo build --release -p rui-client-desktop --features gtk-source-probe --example gtk_source_probe --locked
python3 fixtures/gtk/mirror_check.py --scenario composition --xvfb "$RUI_XVFB" --output /tmp/rui-native-composition-NEW
```

This exercises the native controller and observer, GTK composition and guarded edits through the shared session actor. It checks actual after-paint mirror frames against captured native source content using the declared renderer comparison criteria, independently audits the command trace, and requires zero host-owned memory after disposal. Evidence is in the output directory, including `interaction.json`.

For a simpler native launcher smoke test, use `python3 demos/check.py --scenario local --xvfb "$RUI_XVFB"` after the first build above. For browser engine coverage without a GTK host, see the [three-engine WASM, renderer and installed-package commands](docs/browser-ci.md#reproducible-browser-checks). Performance measurements and their gates are described in [performance](docs/performance.md), [large workloads](docs/large-performance.md) and [browser lifecycle measurements](docs/browser-lifecycle-performance.md).

These demonstrations cover two cooperating applications and one authoritative writer. Two physical tailnet machines, physical mobile browsers and actual screen-reader usability remain unverified. The original action-latency and large-scene 60 Hz targets remain unmet; see the [implementation record](docs/implementation-plans.md#implementation-record-2026-10-10).

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
- [client-wasm](client-wasm/README.md): Rust receiver/session bindings with generated TypeScript declarations, verified in Node, Chromium, Firefox and Playwright WebKit.
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

The owned host can provision a second approved identity with `--second-client`.
Its separate file is `second-client.credential`, for instance ID
`04040404040404040404040404040404`; the original uses ID `03` repeated sixteen
times. Both can request the single control lease, so release before handoff. These
files are caller-owned secrets; keep them out of URLs and bundles. With this option,
the host's local stdin console accepts `restart` (bounded owned source replacement)
and `observer-first` (revoke/reprovision client 03 as read-only). The latter replaces
its credential file; reload it explicitly before reconnecting. Commands are local
operator actions and have no network endpoint. Enter still stops the demo.
`--application editor --partial-collection` materializes two of ten real owned
documents and advertises a partial collection.
