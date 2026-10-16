# Browser workspace

To attach to existing native applications instead, use the
[desktop workspace](../desktop/README.md).


A visible Chromium browser runs on the Linux host. A responsive web client controls its tabs and standard page controls from a phone or another computer. Chromium's profile, cookies, uploads and downloads stay on the host. This app uses RUI's semantic transport: the client receives text and controls, not a screenshot or a copy of the page's JavaScript.

## Start locally

See the shared [prerequisites](../../docs/getting-started.md#prerequisites) for the tested Rust/Node versions and build tools.
This app additionally needs Chromium, the WASM target, and `wasm-bindgen-cli` 0.2.129; GTK is not required.
Use a graphical Linux session with its `WAYLAND_DISPLAY` or `DISPLAY` environment; `--headless` is for automated tests.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
python3 apps/browser/run.py --http-local --files-dir "$HOME/Downloads"
```

The launcher builds Rust, WASM and web assets and starts a dedicated visible Chromium with a persistent profile. Open the printed invitation URL in a **separate client browser**, compare its pairing code with the terminal, and type `approve CODE` on the host. Choose **Take control**, enter an address, and choose **Go**. Subsequent launches can use `--skip-build` until the code changes. `--chromium /path/to/chromium` overrides the executable.

The default state directory is `~/.local/state/rui/browser` (mode 0700). It contains the Chromium profile, device approval digests, internal TLS identity, and downloads. `--state-dir /path` selects another workspace; only one host can use a workspace at a time. Stop with `quit` or Ctrl+C. Persistent login cookies survive restarts; session-only cookies follow Chromium’s normal session behavior.

## Connect through Tailscale Serve

Start the app with the external HTTPS URL of this host:

```sh
python3 apps/browser/run.py \
  --public-url https://foo.tailnet.ts.net/bart \
  --port 8444 --files-dir "$HOME/Downloads" --skip-build
```

In another terminal, configure Tailscale Serve:

```sh
tailscale serve --bg --https=443 --set-path=/bart http://127.0.0.1:8444
tailscale serve status
```

Replace `foo.tailnet.ts.net` with this machine’s full Tailscale DNS name, matching the URL printed by Serve. `--public-url` declares the app’s external URL, including an optional path prefix; it does not create or rename a DNS entry. A bare value such as `foo.tailnet.ts.net/bart` also works and assumes HTTPS. Omit `--skip-build` on the first run or after changes to compiled assets.

[Tailscale Serve](https://tailscale.com/docs/reference/tailscale-cli/serve) handles the phone-facing HTTPS certificate and proxies HTTP/WebSocket traffic to the app’s loopback listener. Both devices must have access through the tailnet. Enable HTTPS when prompted by Tailscale if it is not already enabled.

The app defaults to `127.0.0.1:8444` in this mode. Do not add `--http-local`, `--cert`, `--key`, `--host`, or `--listen 0.0.0.0`. The QR invitation uses the public HTTPS URL, including its public port if specified; `--port` always controls the separate local listener. The example serves the app at `/bart/`; `/bart` works too. Tailscale strips the mount prefix before proxying, so keep the backend target at `http://127.0.0.1:8444` without `/bart`. Assets, pairing, and WSS use the external prefix. Nested paths such as `/tools/bart` work; paths use plain letters, digits, `.`, `_`, `~`, and `-`, with no dot segments or percent escapes. To serve at `/`, omit the path from `--public-url` and omit `--set-path`. Host and Origin checks remain bound to the configured public URL, and device approval is still required. Forwarded identity headers do not grant control. Remembered device credentials are keyed by path, so separate workspaces on the same hostname do not reuse approvals; changing the mount path requires pairing there again. Paths are not a browser security boundary between mutually untrusted applications.

Scan the app’s QR invitation on the phone, compare the pairing code, type `approve CODE` in the host terminal, and choose **Take control**.

## Direct HTTPS over LAN or tailnet

Use an HTTPS certificate trusted by the phone and matching the hostname. The hostname must resolve to this host from the phone. The app serves HTTPS and WSS on the same port; it does not provision DNS, certificates, a relay, or firewall rules.

```sh
python3 apps/browser/run.py \
  --host HOSTNAME --listen 0.0.0.0 --port 8444 \
  --cert /path/to/fullchain.pem --key /path/to/private-key.pem \
  --files-dir "$HOME/Downloads"
```

Scan the terminal QR code in iOS Safari or Android Chrome. Confirm the displayed code on the host with `approve CODE`, then take control on the phone. The invitation expires in five minutes; type `invite` for another. A pending approval expires in two minutes. Direct HTTP is restricted to loopback development. The Tailscale Serve mode above uses loopback HTTP behind its HTTPS proxy.

Approvals are remembered in the client's IndexedDB and on the host. Terminal commands:

- `devices`: show approved client IDs.
- `revoke CLIENT_ID`: invalidate a device's approval and active authorization.
- `deny CODE`: deny a pending request.
- `invite`: print a fresh invitation URL and QR code.
- `quit`: stop the workspace and its owned Chromium.

**Forget device** removes the local credential; use `revoke` on the host to remove its approval too. Use a fresh QR invitation to pair again. A browser reload or reconnection requires taking control again. Only one client controls the host at a time.

## Using the workspace

- Tabs, address navigation, back/forward/reload, and links operate on the real host browser. Popups become tabs. Local host changes are observed too.
- Text, Unicode/IME composition, plain contenteditable, checkboxes, radio buttons, native selects, date/number fields, and JavaScript dialogs use semantic controls. Typing remains a local draft until **Apply**. Changed source fields invalidate old drafts; **Discard** loads the latest value.
- Passwords are blank in publications. Applying a password stages it through an authenticated HTTPS endpoint, then sends a single-use token bound to the device, control lease, epoch and source target. The token expires after 60 seconds. The source website receives the password normally. The input clears on submit, loss of control, and backgrounding. Native browser credential prompts, passkeys and browser extension UI require host interaction.
- Uploads choose a host file inside `--files-dir`. Folder traversal and symlinks outside that root are rejected. Uploading from the phone, directory uploads, and selecting multiple files are not implemented.
- Downloads remain in `STATE/downloads`; the client shows their completion and host path. It can cancel an active download, but cannot download that file to the phone.
- Headings, paragraphs, lists, tables and document regions reflow through mobile HTML templates. This is a structured reading view, not a copy of site CSS or JavaScript; authenticated actions still execute in the host browser.
- Long documents are presented in sections of 400 semantic entries. **Scroll host page down** can trigger lazy-loaded content. The source browser retains its desktop viewport while the phone layout reflows.
- An action receipt means **dispatched**, not that a website has completed its work. Page state arrives separately. Pending commands can be explicitly retried with their original identity; gestures are never automatically replayed. A full page reload abandons pending client results, but continues above the host's command high-water mark so it cannot reuse an old command identity.

## Current limits

This is a first functional workspace, not general browser parity. Images are represented by descriptions when available; canvas, video, WebGL, visual CAPTCHA, maps, rich editors, drag/drop, custom controls without accessible actions, browser chrome/settings and extension UI are unavailable. Some sites reject synthetic semantic clicks or require native trusted input. Unsupported or inaccessible frame content is marked partial. Browser sandboxing stays enabled.

The initial host target is Linux Chromium. The client has been exercised at 390 × 844 with touch enabled in Chromium, Firefox and WebKit on Linux. **Physical iOS Safari and Android Chrome acceptance is still pending.** Desktop WebKit is not proof of iOS behavior. Background suspension, real mobile keyboards/IME, certificate trust and camera QR scanning must be checked on devices.

Bounds include 16 tabs, 16 frames per capture, 16 staged passwords, 16 KiB field values, 128 file entries per directory, 64 download records, and a bounded semantic publication. Limits reject or omit unsupported work instead of falling back to pixels. This is intended for a reachable LAN/tailnet host; there is no internet rendezvous service.

## Verification

Build once with the launcher, then:

```sh
npm --prefix apps/browser test
npm --prefix apps/browser run test:live
npm --prefix apps/browser run test:proxy
npm --prefix apps/browser run test:proxy:path
npm --prefix apps/browser exec -- playwright install firefox webkit
RUI_CLIENT_ENGINE=firefox npm --prefix apps/browser run test:live
RUI_CLIENT_ENGINE=webkit npm --prefix apps/browser run test:live
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 conformance/check.py
```

Playwright's WebKit requires its platform system libraries. `RUI_CLIENT_EXECUTABLE` can point to a compatible client-browser executable/wrapper. The host Chromium executable remains separately selected by `RUI_CHROMIUM`. Tests use temporary profiles and self-signed local HTTPS; their client certificate bypass is confined to tests. JSON reports and screenshots are written to ignored `test-output/` and explicitly state `physicalDevice: false`.

The proxy test runs the live flow through a local TLS-terminating reverse proxy with a different backend port, preserving Host and Origin as Tailscale Serve does. It also checks rejection of an incorrect Host or Origin and spoofed forwarding headers. The path variant strips `/bart` as Tailscale Serve does and verifies that JS/CSS/WASM, API requests, and WebSockets remain under the mount, including a bare `/bart` entry and remembered-device reload. It also checks that an existing root-path credential is untouched. This tests proxy compatibility; it does not change the machine’s Tailscale configuration.

The worker fixture checks Unicode edits, stale source and navigation rejection, password redaction/single-use/expiry/lease binding, upload path confinement, native form controls, embedded frames, dialogs, downloads and tabs. The live test exercises production assets through the Rust host, HTTPS/WSS, local approval, control, navigation, edits, password entry, release, remembered-device reconnect, host restart with a persistent login cookie, and host revocation.

Physical phone acceptance: pair through a trusted HTTPS QR invitation; navigate and switch tabs; complete a standard login form; edit with the OS keyboard and IME; upload a host file and download to the host; background/foreground and disconnect/reconnect; change the same field locally on the host while drafting; revoke the phone on the host and confirm it can no longer act. Repeat on iOS Safari and Android Chrome before calling mobile support validated.

## Implementation

`run.py` owns the process group and profile lock. `rui-browser-host` owns authentication, control leases, the command ledger, source validation and RUI publications. The private Node worker owns Playwright/CDP and serves the static mobile app plus the same-origin WSS proxy. Private mode-0600 Unix sockets connect the worker and Rust host; the proxy verifies the internal RUI TLS identity. Public APIs expose pairing and password staging, never CDP, selectors or arbitrary script execution.

The negotiated `browser.semantic.v1` profile adds optional input-type metadata and a reconnect command-sequence floor. Browser actions terminate with `Dispatched`; publication does not falsely claim an atomic DOM snapshot or causal completion. Existing GTK/vector profiles retain their existing command completion behavior.
