# Getting started

Use the [desktop workspace](../apps/desktop/README.md) to share applications already running in your Linux session.
Use the [browser workspace](../apps/browser/README.md) for a dedicated host Chromium, or the cooperating GTK demos below to compare semantic and vector presentations.
All commands run from the repository root.

## Prerequisites

The tested build toolchains are Rust 1.98.1 and Node 26.8.1 with npm.
The browser library requires Node 26.8.1 or newer within 26.x.
Install Python 3.11 or newer, `protoc`, a native compiler toolchain, and OpenSSL.
Run visible applications from a graphical Linux session with `WAYLAND_DISPLAY` or `DISPLAY` available.

Browser clients also require the WASM target and the bindings CLI matching [client-wasm/Cargo.toml](../client-wasm/Cargo.toml):

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
wasm-bindgen --version
```

The final command must report `wasm-bindgen 0.2.129`, and the executable must be on `PATH`.
If installed elsewhere, set `WASM_BINDGEN=/absolute/path/to/wasm-bindgen`; the GTK browser launcher also accepts `--wasm-bindgen /absolute/path/to/wasm-bindgen`.
Launchers build the required Rust/WASM/web assets and install locked npm dependencies on the first run.
Use `--skip-build` on later runs after the selected app or demo has been built, until code changes.

Additional source requirements:

| Workspace or demo | Native requirements |
| --- | --- |
| Existing desktop apps | Python PyGObject/Gio, a running AT-SPI accessibility bus/registry, and apps exposing accessibility; GTK4/PyGObject for the graphical sharing picker |
| Managed browser | Chromium and an existing host files directory, defaulting to `~/Downloads`; GTK is not required |
| Cooperating GTK demos | `pkg-config`, GTK 4.22.4/X11 development files, Cairo 1.18.4, Python PyGObject/PyCairo, and OpenSSL; Xwayland works on a Wayland desktop |

The pinned native renderer environment is documented in the [renderer profile](../spec/renderer-profile.md) and [GTK coverage report](gtk-coverage.md).
Automated native checks additionally need Xvfb, `xauth`, `dbus-run-session`, libX11, and libXtst/XTest; see [development and verification](development.md).

## Existing apps and managed browsing

For local existing-app sharing, follow the [README quick start](../README.md#try-it).
The [desktop workspace guide](../apps/desktop/README.md#run) covers phone HTTPS/Tailscale setup, the sharing picker, terminal-only sharing, and device approval.
Its compatibility matrix distinguishes tested Writer/Nautilus workflows from broader application coverage.
Add `--with-browser` to include a managed Chromium in the same shared session.

For the standalone browser workspace:

```sh
python3 apps/browser/run.py --http-local --files-dir "$HOME/Downloads"
```

Open the printed invitation in a separate client browser, compare the pairing code, type `approve CODE` on the host, and choose **Take control**.
Enter an address and choose **Go**.
Use `--chromium /path/to/chromium` if needed; `--files-dir` must name an existing directory.
The [browser guide](../apps/browser/README.md) covers phone access, persistent profiles, forms, passwords, uploads/downloads, and revocation.

These workspaces remember device approvals in their state directories and stop with `quit` or Ctrl+C.
Attached native apps keep running; the managed Chromium closes with RUI.
Existing-app sharing exposes accessible controls with observed, partial semantics, rather than a faithful visual mirror.

## Cooperating GTK demos

These demos use owned GTK applications with stronger source guards and temporary credential files.
Their browser clients pair by selecting the printed file, rather than the workspace invitation flow.
Run the browser launchers one at a time: both presentations use port 5173.

### Semantic editor

```sh
python3 demos/web/run.py --presentation semantic --application editor
```

The launcher opens the real GTK document editor and prints the web URL and `client.credential` path.
Open that URL, choose the credential file, click **Connect**, then **Take control**.

1. Under **Tasks** or **Field notes**, choose **Select source item** and watch the GTK selection and fields follow.
2. On **Body**, choose **Edit draft**, enter `café Ω 𝄞 é שלום`, and choose **Apply draft**.
   The native field shows the committed string; local typing remains a draft until applied.
3. Use **Activate** under **Save document**, toggle **Reviewed**, and activate **Document details** and **Close details**.
   Source status, Boolean state, and the modal relationship update in both views.
4. Narrow the browser window to see its layout reflow while the source keeps its layout.
   The private field stays redacted.
5. Start another draft, change that source field in GTK, then apply the old draft.
   The client reports a conflict and preserves the draft.
6. Disconnect and reconnect, then explicitly take control again.

Close the host or press Ctrl+C in the launcher terminal to stop.
Use `--application corpus` for the original counter/text/list application.
The [semantic presentation guide](../demos/semantic/README.md) explains controls, accessibility, and lifecycle behavior.

### Browser vector mirror

```sh
python3 demos/web/run.py
```

Connect with the printed credential and take control as above.
Activate a mirrored button, or click a text field and use **Apply to host** in its editor.
Host-side changes appear in the browser.
Use `--application editor` to compare the editor's source-layout vectors with its semantic presentation.
Returning from a hidden tab requires taking control again.

To serve on tailnet machine A for a browser on B:

```sh
python3 demos/web/run.py --skip-build --public-host HOST_A --listen 0.0.0.0:8443
```

Open `http://HOST_A:5173` on B and choose a securely copied version of A's printed credential.
B needs only a browser.
The frontend proxy verifies the Rust host certificate and preserves the exact Origin allowlist.
This is the demo's HTTP development mode; [the browser vector guide](../demos/web/README.md) covers HTTPS/WSS, separate processes, static output, and capture limits.

### Native host and vector client

With the native GTK prerequisites, no Node or WASM setup is needed:

```sh
cargo run --release -p rui-client-desktop --features gtk-demo --bin rui-demo --locked
```

**RUI Host** is the actual GTK application; **RUI Client** renders its published state and sends commands over TLS.
Increment the counter, edit text, or select a list row in either window and watch the other follow.
The client offers control, zoom, reconnect, and cache controls; request control again after reconnecting.
Close either window to stop the local demo.
The launcher removes its private temporary TLS identity and controller credential on normal shutdown.
Manual demos have no scripted edits or automatic test timeout.

### Native host and client on separate machines

On tailnet machine A, start the host with a new identity directory:

```sh
cargo run --release -p rui-client-desktop --features gtk-demo --bin rui-demo --locked -- --host-only --listen 0.0.0.0:8443 --identity-dir /tmp/rui-demo
```

Keep A running and copy only `host.der` and `client.credential` to B, replacing `USER` and `HOST_A`:

```sh
scp USER@HOST_A:/tmp/rui-demo/host.der /tmp/rui-demo-host.der
scp USER@HOST_A:/tmp/rui-demo/client.credential /tmp/rui-demo-client.credential
```

Start the native client on B with the native prerequisites installed:

```sh
GDK_BACKEND=x11 GTK_IM_MODULE=simple cargo run --release -p rui-client-desktop --features desktop --bin rui-mirror --locked -- HOST_A:8443 rui-demo /tmp/rui-demo-host.der /tmp/rui-demo-client.credential 03030303030303030303030303030303 --control --scale=0.75
```

`rui-demo` is the certificate's TLS name; the network address is supplied separately.
The credential grants one controller permission to view and edit the demo app.
Keep private keys on A, and copy fresh client files after each host restart.
Close the host or press Enter in A's terminal to stop; disconnecting B leaves A running for reconnection.
Without `--listen`, the listener uses an ephemeral loopback port.
Two-physical-machine acceptance remains a separate test boundary, as recorded in [current status](status.md).

### Additional host controls

The native host supports `--second-client`, creating `second-client.credential` for instance ID `04040404040404040404040404040404`.
The original client ID is `03` repeated sixteen times.
Both approved clients can request the single control lease; release it before handoff.
Credentials are caller-owned secrets and stay out of URLs and bundles.

With `--second-client`, the local stdin console accepts `restart` for bounded source replacement and `observer-first` to revoke/reprovision client 03 as read-only.
Reload the replaced credential explicitly before reconnecting.
These are local operator commands with no network endpoint; Enter still stops the host.
`--application editor --partial-collection` materializes two of ten owned documents and advertises a partial collection.
