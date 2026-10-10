# Reproducible browser checks

`.github/workflows/browser.yml` builds the real Rust/WASM artifact from a clean
checkout with Rust 1.98.1, the WASM target, Node 26.8.1 and locked npm installs.
It reads the exact wasm-bindgen version from the binding crate, installs the
matching CLI, and never depends on generated files or `/tmp` executables.

The portable job runs the same native-oracle WASM trace in Node and Chromium,
Firefox and Playwright WebKit, plus browser pixel/resource/lifecycle checks.
Each runner prints the actual engine version. Production checks serve the built
Vite bundle, fetch and compile its hashed WASM asset with the correct MIME type,
and exercise successful initialization followed by a bounded transport failure.
This asset check is separate from live host interaction.

The live job requires a runner labeled `rui-gtk-4.22.4` with GTK 4.22.4, Cairo
1.18.4, PyGObject/PyCairo, protoc, pkg-config, libatomic, OpenSSL, Xvfb, xauth and
dbus-run-session. The job verifies the native versions before building. Both
development and production serving use private authenticated displays, an owned
bus, loopback TLS and an exact certificate trust exception. They check host and
browser edits, Unicode, observer denial, reconnect and actual hidden-tab control
release. The existing Rust transport test
`browser_origin_is_exact_opt_in_and_compression_is_never_selected` checks exact
Origin rejection in the original core job.

Provision that pinned runner before enabling required live-job checks. The
portable Ubuntu job does not imply that Ubuntu's GTK implements this adapter.
Neither viewport emulation nor Playwright WebKit establishes Safari or mobile
support. Native acceptance and independent replay remain separate enabled jobs.

For local execution after building WASM and installing locked dependencies:

```sh
npm --prefix client-wasm exec -- playwright install chromium firefox webkit
RUI_TEST_BROWSERS=chromium,firefox,webkit npm --prefix client-wasm test
RUI_TEST_BROWSERS=chromium,firefox,webkit npm --prefix client-browser test
npm --prefix demos/web run build
RUI_TEST_BROWSERS=chromium,firefox,webkit node demos/web/tests/production.mjs
```

`RUI_WASM_CHROMIUM`, `RUI_WASM_FIREFOX` and `RUI_WASM_WEBKIT` optionally select
owned executable paths. Failure artifacts retain bounded numeric timing reports
for seven days; credentials, private keys and source capture text are excluded.

The [retained local report](../fixtures/browser/ci-local.json) records clean source
builds and suites in Chromium 153.0.8010.12, Firefox 155.0 and Playwright WebKit
26.6. The workstation WebKit run used isolated compatibility libraries; the
Ubuntu CI job installs those dependencies through Playwright. Both live GTK
serving modes passed headed HTTPS and hidden-tab checks.
