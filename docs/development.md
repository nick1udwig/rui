# Development and verification

See [getting started](getting-started.md#prerequisites) for toolchains and native prerequisites.
Run commands from the repository root.
Choose checks for the component you changed; the live suites exercise separate source and transport boundaries.

## Core checks

The workspace checks require Rust 1.98.1, `protoc`, Python 3, `pkg-config`, and Cairo 1.18.4 development files.

```sh
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 conformance/check.py
python3 -m unittest adapters.sdk.test_producer
```

Transport tests need loopback socket access, and the CPU renderer checks its native Cairo version.
CI builds checksum-pinned Cairo into an isolated prefix with [ci/build-cairo.sh](../ci/build-cairo.sh), which needs Meson, Ninja, and Pixman development files.
The Python conformance check needs no third-party Python packages.
It independently decodes exact Protobuf fixtures and compares Rust/Python state after accepted and rejected updates, including all nine scene primitives.
The workspace lockfile pins Rust dependencies; toolkit handles do not belong in the wire schema.

## Workspace checks

For existing-app sharing, use the [desktop verification guide](../apps/desktop/README.md#verification).
Its consolidated runner covers native guards, mobile Chromium/WebKit flows, builds, and the browser workspace regression:

```sh
python3 apps/desktop/tests/check.py --xvfb /path/to/Xvfb
```

Add `--real-desktop` only for the documented Writer/Nautilus checks in a running Wayland session.
The [browser workspace guide](../apps/browser/README.md#verification) documents its unit and live tests.
Physical-device acceptance remains distinct from emulated mobile viewports.

## Live cooperating GTK checks

These checks start owned GTK apps and clients on private authenticated Xvfb displays and a private session bus.
Install the pinned GTK/X11 environment, Xvfb, `xauth`, `dbus-run-session`, libX11, and libXtst/XTest.
Run live checks sequentially with manual launchers stopped so port 5173 is free.
Do not rebuild browser/WASM assets during a live check.

Prepare the artifacts:

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

If Xvfb is outside `PATH`, set `RUI_XVFB` to its absolute path.
Set `RUI_WASM_CHROMIUM=/path/to/chromium` to use a full installed Chromium instead of the Playwright download.
Playwright also needs its system libraries; [browser CI](browser-ci.md) documents the tested environment.
Replace `NEW` below with a fresh suffix because presentation and native-capture output directories must not contain a previous run.

### Semantic controller and vector observer

```sh
RUI_RESOURCE_RECOVERY=1 python3 demos/semantic/presentation_check.py --application editor --xvfb "$RUI_XVFB" --output /tmp/rui-presentation-editor-NEW
RUI_RESOURCE_RECOVERY=1 python3 demos/semantic/presentation_check.py --application corpus --xvfb "$RUI_XVFB" --output /tmp/rui-presentation-corpus-NEW
```

Two approved identities swap one control lease and exercise buttons, Unicode edits, and stable-item selection through both presentations.
Checks cover stale drafts, delayed results, resource loss, reconnect, source restart, and reduced permissions; the editor adds partial collections, a modal, and control replacement.
Exact authorized packets replay through the independent Python oracle.
The semantic client receives no Scene/Binding records or glyph resources and sends no visual presentation acknowledgments.
Reports include `semanticOnly.json` and `mirror.json` in each output directory.

### Production browser mirror

```sh
RUI_BROWSER_HEADED=1 RUI_BROWSER_HTTPS=1 RUI_BROWSER_BUILD=1 python3 demos/web/check.py --xvfb "$RUI_XVFB" --timeout 120
```

Built Vite output is served through HTTPS and a certificate-verifying WebSocket proxy.
Headed Chromium checks observer restrictions, browser/native edits, Unicode, reconnect, actual hidden-tab control release, and shutdown cleanup.
The screenshot and report are `client-browser/test-output/demo.png` and `timings.json`.
This correctness check does not establish a latency budget.

### Installed browser package

```sh
RUI_BROWSER_HEADED=1 python3 client-browser/tests/package-live.py --xvfb "$RUI_XVFB"
```

The check packs the private runtime, installs it outside the checkout, type-checks/builds the consumer, and connects that installed code to the real host.
It exercises production HTTPS under `/apps/rui/`, default/replacement adapters, guarded edits, reconnect, browser back/forward cache restoration, and disposal.
The report is `client-browser/test-output/package-live.json`.
The first run needs npm registry access; a warm cache can use `RUI_PACKAGE_OFFLINE=1`.
Add `RUI_LIFECYCLE=1` for 25 connect/disconnect/dispose cycles, resource recovery, delayed acknowledgments, and hidden-tab checks, reported in `client-browser/test-output/lifecycle.json`.

### Native mirror and IME

```sh
cargo build --release -p rui-client-desktop --features gtk-source-probe --example gtk_source_probe --locked
python3 fixtures/gtk/mirror_check.py --scenario composition --xvfb "$RUI_XVFB" --output /tmp/rui-native-composition-NEW
```

This checks native controller/observer composition and guarded edits through the shared session actor.
Actual after-paint mirror frames are compared with captured source content under the declared renderer criteria.
The command trace is independently audited and disposal requires zero host-owned memory.
Evidence includes `interaction.json` in the output directory.

For launcher lifetime and interaction checks:

```sh
python3 demos/check.py --xvfb "$RUI_XVFB"
```

These cover local and separate-process clients, rapid text edits, idle publication stability, and cleanup when either window closes.
Add `--scenario local` for a smaller launcher smoke check.
Optional native mirror probes and build commands are documented in the [renderer profile](../spec/renderer-profile.md).
The retained [two-mirror report](../fixtures/render/native-corpus.json) contains source/mirror hashes and scenario results without document text or credentials.

## Portable browser and performance checks

[Browser CI](browser-ci.md#reproducible-browser-checks) lists the three-engine WASM, renderer, installed-package, and production-asset commands that do not need a live GTK host.
Performance procedures and limits are in [performance](performance.md), [large workloads](large-performance.md), [browser lifecycle measurements](browser-lifecycle-performance.md), and [frozen gates](performance-gates.md).
The original action-latency and large-scene 60 Hz targets remain unmet.
The [case audit](../conformance/CASES.md) maps the accepted integration, visual, recovery, and hardening evidence for the original prototype.

## Small component examples

```sh
cargo run -p rui-host-broker --example controlled
cargo run -p rui-producer-sdk --bin replay_capture -- fixtures/gtk/golden
cargo run -p rui-client-desktop --example reference -- /tmp/rui-reference
cargo run --release -p rui-client-desktop --example render_capture -- fixtures/gtk/golden/main-content-0001.json /tmp/rui-gtk.pam
```

The [documentation index](README.md#components-and-integration) links each component's API and integration guide.
Unsupported required variants fail explicitly; measured cooperating GTK content does not establish arbitrary-app or whole-desktop coverage.
