# Visible GTK host, browser client

The GTK window on machine A owns the counter, text, list and menu. The browser displays its published vector canvas and sends commands back to that window. This Vite app is a consumer of [the reusable client library](../../client-browser/README.md).

## Run it

Install the shared [GTK demo prerequisites](../../docs/getting-started.md#prerequisites), then add the WASM Rust target and matching bindings CLI:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
python3 demos/web/run.py
```

The launcher builds the Rust host and WASM module, installs locked npm dependencies, builds the library, and starts the visible host plus Vite on `0.0.0.0:5173`. Open the printed URL. Choose the printed `client.credential` file, click **Connect**, then **Take control**. Click **Increment host counter** in the mirror or click a text field to open its draft editor. **Apply to host** or Ctrl+Enter commits the draft. Host-side edits appear in the browser as well.

A client starts as an observer even with a controller credential. Returning from a hidden tab or reconnecting requires **Take control** again. Stale editing contexts reject; the client does not overwrite a newer host edit automatically. Closing the host or pressing Ctrl+C stops the launcher and removes its private temporary identity directory. Reuse built artifacts and installed dependencies on later runs:

```sh
python3 demos/web/run.py --skip-build
```

Set `WASM_BINDGEN` or pass `--wasm-bindgen /path/to/wasm-bindgen` if the CLI is installed separately. The demo credential is the temporary file printed by this host launch; transport test credentials belong to their own fixtures.

## Tailnet machine A → browser on machine B

On A, replace `HOST_A` with the hostname or IP B will put in its browser URL:

```sh
python3 demos/web/run.py --skip-build --public-host HOST_A --listen 0.0.0.0:8443
```

Vite serves the browser app on all interfaces. Its `/rui` proxy connects to the Rust TLS host and verifies the fresh host certificate and the `rui-demo` TLS name. It forwards exact binary packets and the original browser Origin; the Rust server only accepts explicitly allowed Origins. The credential is not embedded in JavaScript or served by an HTTP endpoint.

Copy **only the printed `client.credential` file** to B using your existing SSH/tailnet connection. Open `http://HOST_A:5173` on B and choose that copied file. B needs a browser, not Rust or GTK. Keep both private keys on A. One approved demo client instance can be connected at a time; disconnect the local browser before connecting B.

HTTP/WS is the explicit development mode. For HTTPS/WSS, supply a certificate trusted by B's browser for the hostname in the URL and its key:

```sh
python3 demos/web/run.py --skip-build --public-host HOST_A --listen 0.0.0.0:8443 --cert /path/to/HOST_A.crt --key /path/to/HOST_A.key
```

Open `https://HOST_A:5173` on B. Browser certificate verification protects this front connection; the proxy separately verifies the Rust host certificate. Origins include the scheme, hostname and port and must match the actual browser URL. The library also supports a direct `wss:` host with a browser-trusted certificate; browsers cannot use the native client's arbitrary DER pinning API.

## Separate processes and static output

Build without a running host:

```sh
python3 client-wasm/build.py
npm --prefix client-browser ci
npm --prefix client-browser run build
npm --prefix demos/web ci
npm --prefix demos/web run build
```

`demos/web/dist/` contains the HTML, CSS, ES module and WASM asset. Serve that output over trusted HTTPS with a `/rui` binary WebSocket proxy to the TLS host. Keep certificate verification enabled on the backend. Vite's preview server exercises the built output locally; it is not a production deployment server.

To launch the host and development server manually in separate terminals:

```sh
cargo run --release --locked -p rui-client-desktop --features gtk-demo --bin rui-demo -- --host-only --listen 0.0.0.0:8443 --identity-dir /tmp/rui-browser-demo --browser-origin http://localhost:5173 --browser-origin http://127.0.0.1:5173
RUI_DEMO_IDENTITY=/tmp/rui-browser-demo npm --prefix demos/web run dev
```

The identity directory must be new. For another hostname, add its exact Origin to the host command and set `RUI_WEB_HOSTS=HOST_A` for Vite. `RUI_HOST_URL` defaults to `wss://127.0.0.1:8443`; `RUI_WEB_CERT` and `RUI_WEB_KEY` configure HTTPS. These are server configuration variables and are not bundled browser secrets. See [Vite server options](https://vite.dev/config/server-options) for hosting/proxy configuration.

## Validation and current limits

The source exporter remains the constrained cooperating GTK adapter. Color emoji, raster content and unsupported native paint nodes can make a view incomplete. The demo disables blinking/animations, and the exporter recognizes fully opaque/hidden caret wrappers; partial source opacity remains outside its accepted capture scope. Browser text drafts support normal DOM editing, including paste/deletion and IME-produced committed strings, while the host receives a guarded semantic replacement. Native source preedit/raw input are separate future browser features.

The host uses bounded source dirty notifications and publishes incremental transactions for warm edits, with snapshots for initial connection and recovery. Browser rendering reuses vector coverage and pixel buffers with explicit linear composition. The original action-latency and large-scene 60 Hz targets remain unmet; see [performance](../../docs/performance.md). Draw timing appears in the footer. Cross-machine binding is supported; the live tests bind `0.0.0.0` and connect over loopback, rather than claiming a second physical machine was tested.

```sh
cargo build --release --locked -p rui-client-desktop --features gtk-demo --bin rui-demo
npm --prefix client-wasm ci
npm --prefix client-wasm test
npm --prefix client-browser test
python3 demos/web/check.py --xvfb /path/to/Xvfb
```

The integration harness uses an authenticated private Xvfb display and an owned browser, never the user's desktop. It checks observer/controller behavior, browser activation, Unicode edits, host edits, reconnect and clean host shutdown. To check real hidden tabs and the built HTTPS/WSS output, use a full Chromium executable:

```sh
RUI_BROWSER_HEADED=1 RUI_BROWSER_HTTPS=1 RUI_BROWSER_BUILD=1 RUI_WASM_CHROMIUM=/path/to/chromium python3 demos/web/check.py --xvfb /path/to/Xvfb
```

The HTTPS fixture trusts only its freshly generated certificate's public key in that owned test browser. The manual HTTPS path uses the browser's normal trust store.
