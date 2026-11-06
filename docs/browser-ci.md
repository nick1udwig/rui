# Reproducible browser checks

`.github/workflows/browser.yml` builds the real Rust/WASM artifact from a clean checkout with Rust 1.98.1, the WASM target, Node 26.8.1 and locked npm installs.
It reads the exact wasm-bindgen version from the binding crate, installs the matching CLI, and never depends on generated files or `/tmp` executables.

The portable job uses the digest-pinned Playwright 1.63.0 Noble container with Chromium, Firefox, WebKit, and their OS dependencies already installed.
It checks that the locked Playwright package matches the image version.
This avoids downloading the full browser OS dependency set from the hosted runner's apt mirror during each job.

The portable job runs the same native-oracle WASM trace in Node and Chromium, Firefox and Playwright WebKit, plus browser pixel/resource/lifecycle checks.
Each runner prints the actual engine version.
Production checks serve the built Vite bundle, fetch and compile its hashed WASM asset with the correct MIME type, and exercise successful initialization followed by a bounded transport failure.
This asset check is separate from live host interaction.

The live job runs on a hosted Ubuntu runner through `ci/gtk.Dockerfile` and `ci/gtk-check.sh`.
The image builds checksum-verified GTK 4.22.4 against Debian Trixie's Cairo 1.18.4 and verifies both versions before running tests.
Its base images are pinned by digest; Node 26.8.1 is checksum-verified, Rust is 1.98.1, and wasm-bindgen is 0.2.129.
Chromium comes from the same Playwright 1.63.0 image as the portable job.
The container-only launcher disables Chromium sandboxing because tests run as root inside the isolated CI container.
The image provides PyGObject 3.58.1, PyCairo 1.28.0, protoc, pkg-config, libatomic, OpenSSL, Xvfb, xauth, dbus-run-session, and Noto fonts for the Unicode fixtures.
The image build checks the Cairo scaled-font converter used by the source glyph exporter; Debian's older PyGObject does not provide it.
No separately registered self-hosted runner is needed.

The native script retains all live acceptance checks from the previous self-hosted job.
Both development and production serving use private authenticated displays, an owned bus, loopback TLS, and an exact certificate trust exception.
They check host and browser edits, Unicode, observer denial, reconnect, and actual hidden-tab control release.
The existing Rust transport test `browser_origin_is_exact_opt_in_and_compression_is_never_selected` checks exact Origin rejection in the core job.
Neither viewport emulation nor Playwright WebKit establishes Safari or mobile support.

Run the complete native job locally with Docker:

```sh
docker build -f ci/gtk.Dockerfile -t rui-gtk-ci ci
docker run --rm --init --ipc=host -v "$PWD:/workspace" rui-gtk-ci bash ci/gtk-check.sh
```

For local execution after building WASM and installing locked dependencies:

```sh
npm --prefix client-wasm exec -- playwright install chromium firefox webkit
RUI_TEST_BROWSERS=chromium,firefox,webkit npm --prefix client-wasm test
RUI_TEST_BROWSERS=chromium,firefox,webkit npm --prefix client-browser test
RUI_TEST_BROWSERS=chromium,firefox,webkit npm --prefix client-browser run test:package
npm --prefix demos/web run build
RUI_TEST_BROWSERS=chromium,firefox,webkit node demos/web/tests/production.mjs
```

`RUI_WASM_CHROMIUM`, `RUI_WASM_FIREFOX` and `RUI_WASM_WEBKIT` optionally select owned executable paths.
Failure artifacts retain bounded numeric timing reports for seven days; credentials, private keys and source capture text are excluded.

The [retained local report](../fixtures/browser/ci-local.json) records clean source builds and suites in Chromium 153.0.8010.12, Firefox 155.0 and Playwright WebKit 26.6.
The workstation WebKit run used isolated compatibility libraries; the Ubuntu CI job uses the pinned Playwright image.
Both live GTK serving modes passed headed HTTPS and hidden-tab checks.

L5 additionally builds a runtime-only tarball, installs it outside the checkout and tests Vite production assets under `/apps/rui/`, Node import without DOM, declaration compilation, initialization/ABI/MIME/CSP and disposal in all engines.
The pinned GTK job runs `client-browser/tests/package-live.py` against those installed files, through default and replacement adapters with direct WSS and a verifying proxy.
Reports are in `fixtures/browser/distribution`; actual two-machine/device acceptance is separate and unverified.
