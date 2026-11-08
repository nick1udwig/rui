# Browser workspace

To attach to existing native applications instead, use the
[desktop workspace](../desktop/README.md).

A mobile web client displays and controls Chromium on the host.
It can launch its own browser or attach to a browser with a local debugging endpoint.
The client renders page DOM, CSS, text, controls, SVG, images and fonts in an isolated iframe.
Website JavaScript runs only on the host.
Authenticated browser sessions remain on the host.
Canvas, WebGL, video and audio use separate element-scoped media streams.
Unloaded videos retain their poster images.
The app never sends a full-page raster as its page representation.

## Try it on your phone

Keep Tailscale installed, signed in and connected on both this computer and your phone.
From the repository root, run:

```sh
python3 apps/browser/run.py --phone
```

The first run builds the demo and opens a host Chromium window.
Scan the printed QR code with your phone.
Approve the phone once by typing the printed `approve CODE` command in the host terminal, then tap **Take control** on the phone.
Enter a website address in the mobile app to try it.
No PEM files, hostname selection or separate proxy command is needed.

The launcher selects unused ports and starts a temporary [Tailscale Serve](https://tailscale.com/docs/reference/examples/serve) HTTPS route.
It leaves existing Serve routes unchanged and removes its own route when you quit or press Ctrl+C.
If Tailscale asks you to enable HTTPS for your tailnet, follow its printed setup link once and run the command again.
Use `--skip-build` on later runs when the code has not changed.
The managed browser uses direct ICE addresses so its element media streams can connect over the tailnet.
Phone mode does not add a media relay or change the ICE settings of an externally attached browser.

## Desktop-only development


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

## Attach to a running browser

The browser must already expose a local Chromium debugging endpoint.
A normal browser without that endpoint cannot be attached by this adapter.
The endpoint is never exposed to the mobile client.

```sh
python3 apps/browser/run.py --http-local --cdp-url http://127.0.0.1:9222 --files-dir "$HOME/Downloads"
```

Use the HTTPS options below for a physical phone.
Only loopback HTTP or WebSocket CDP endpoints are accepted.
RUI shares up to 16 existing tabs and leaves excess tabs open.
Commands activate the selected source tab.
Taking control lets the client set that tab's emulated viewport to the mobile document size.
RUI preserves the external browser window bounds and clears its viewport override on detach.
Quitting disconnects RUI without closing the external browser or its tabs.

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

- The client looks like a mobile browser.
  A monitor icon in the address bar marks pages that run on the host.
  The icon and an underline under the address bar turn amber while this device controls the host.
  **Connect** and **Take control** appear at the end of the address bar when they apply.
  The menu shows the connection state and **Release control**.
  Without control, a tap on the page or a host control shows a reminder and highlights **Take control**.
  The bottom toolbar has back, forward, page sound, tabs and menu buttons.
  The tabs button shows the host tab count and opens a tab list with new-tab and close buttons.
  The menu also holds the reader view link, page section controls, host downloads and session details.
  Page dialogs and host file selection open as sheets above the page.
- Tabs, address navigation, back/forward/reload, and links operate on the real host browser. Popups become tabs. Local host changes are observed too.
- Text, Unicode/IME composition, plain contenteditable, checkboxes, radio buttons, native selects, date/number fields, and JavaScript dialogs use semantic controls. Typing remains a local draft until **Apply**. Changed source fields invalidate old drafts; **Discard** loads the latest value.
- Passwords are blank in publications.
  Use the password control below the page; password inputs in the page replica are read-only.
  Applying a password stages it through an authenticated HTTPS endpoint, then sends a single-use token bound to the device, control lease, epoch and source target.
  The token expires after 60 seconds.
  The source website receives the password normally.
  The input clears on submit, loss of control, and backgrounding.
  Native browser credential prompts, passkeys and browser extension UI require host interaction.
- Uploads choose a host file inside `--files-dir`. Folder traversal and symlinks outside that root are rejected. Uploading from the phone, directory uploads, and selecting multiple files are not implemented.
- Downloads remain in `STATE/downloads`; the client shows their completion and host path. It can cancel an active download, but cannot download that file to the phone.
- The address bar stays above the page view.
  Enter in the address bar navigates; **Go** also appears while editing.
  The page view fills the space between the address bar and the bottom toolbar.
  The page view uses the site's DOM and CSS with the source viewport matched to the controlling client.
  Large pages defer hidden and distant offscreen DOM sections within the node budget.
  Deferred sections preserve their layout size and load as the controlling phone scrolls the host.
  Visible content stays DOM, CSS, text, images and scoped media.
  During a failed update, the previous page stays visible with an updating message and input is paused.
  Pages that exceed the source node limits show an error and keep retrying; the address bar remains available.
  Fetch failures retain the document and healthy media streams.
  Stalled document requests time out after 15 seconds and retry.
  Video and audio connection errors appear beside the page view.
  Interrupted playback retries, and failed media connections reconnect with increasing delays up to ten seconds.
  A tap on a locally blocked video starts playback on the phone without pausing the host.
  A visible video stream reconnects if host frames advance but no phone frames arrive for eight seconds.
  Retired requests cannot replace a newer view or close its media streams.
  Accessibility refreshes run in the background and publish complete action targets together, so a slow capture does not block the control connection.
  The structured reading controls remain below the page in the reader view.
  Links and controls target stable source nodes.
  Text fields commit on change or Enter; simple rich-text edits commit on blur and preserve unaffected markup.
  Canvas and slider drags send bounded paths to the host.
- Scrolling the page or a nested scroll region also scrolls its source region to trigger lazy loading.
  The separate reading view uses sections of 400 semantic entries.
- An action receipt means **dispatched**.
  Website loading and page updates continue separately.
  **Check action status** retrieves the result of a pending command with its original identity.
  If the host cannot confirm the result, inspect the host page and select **Check host, then reconnect**.
  Take control again after reconnection.
  This clears the pending local result without repeating the action or reusing its command identity.
  Stale or unavailable targets are rejected without blocking later actions.
  Address drafts remain editable while background viewport commands wait for results.

## Current limits

The host adapter targets Linux Chromium.
It is not complete browser parity.
Closed shadow trees, complex editors, multi-touch/pinch, native browser chrome, extension UI, passkeys and protected media are not supported.
CSS imports, blob-backed images and unusual resource formats can be incomplete.
Unsupported frame/plugin regions remain marked instead of becoming a full-page screenshot.
Only open shadow trees and accessible frame documents are replicated.

Media uses encoded WebM video/audio over the same authenticated HTTPS path as the DOM and CSS snapshots.
A persistent binary HTTPS response carries media separately from document updates and control requests.
The phone does not poll or decode base64 media.
The host capture bridge still uses base64 internally.
This transport requires Media Source Extensions with VP8/Opus support.
Browsers without a supported format use direct WebRTC as a compatibility path.
Media elements use this transport; page text, layout and controls remain DOM and CSS.
No additional ports or relay service need configuration.
Use **Enable page sound**, the speaker button in the bottom toolbar, for audio playback.
Media encoding follows the displayed width, with device pixel ratio capped at 1.5 and source resolution preserved as an upper bound.
The default profile caps width at 960 pixels and frame rate at 30 fps.
Sustained decoder pressure reduces the profile to 480 pixels and 15 fps.
Offscreen media stops drawing picture frames while its audio remains connected.
Visible still media repeats its last frame twice per second to keep the encoder and decoder supplied.
Static canvases can start from an initial canvas copy before their capture stream has a frame.
Canvas uses a 100 ms recording interval; video uses 250 ms.
Quality changes restart only the affected media stream, with a 15-second minimum interval.
Media sessions expire without renewal and close when the view is retired or hidden.
When a media element loads a new source, its old stream closes and the phone connects to the new stream.
Desktop WebKit in the test container requires direct ICE addresses because container mDNS discovery did not work.
This test configuration does not validate Safari networking on a phone.

**Physical iOS Safari and Android Chrome acceptance is pending.**
Desktop engine tests use a 390 × 844 viewport and do not prove OS keyboard, IME, touch, background suspension, certificate or QR behavior on a device.

Bounds include 16 tabs, 16 frames, 20,000 transmitted DOM nodes, 8 MiB document packets, an 8 MiB source asset cache and a 16 MiB client resource cache.
Capture scans at most 150,000 source nodes and assigns at most 100,000 node identities per navigation.
There are at most eight media regions per source document and 32 worker media sessions.
Each HTTPS media stream retains at most 4 MiB of unacknowledged data and sends at most 512 KiB per batch.
The server stops a stream if its response remains blocked for five seconds.
Each batch rechecks authorization before delivery.
Large encoder bursts are split across reads without restarting playback.
Stalled or retired streams expire after ten seconds without renewal.
Other existing bounds include 16 staged passwords, 16 KiB field values, 128 host file entries and 64 download records.
A site that exceeds these limits can have omitted assets or a rejected document view.
No internet rendezvous or external TURN relay is provided.

## Verification

Build once with the launcher, then:

```sh
npm --prefix apps/browser test
npm --prefix apps/browser run test:live
npm --prefix apps/browser run test:proxy
npm --prefix apps/browser run test:proxy:path
RUI_TEST_PHONE=1 node apps/browser/tests/live.mjs
npm --prefix apps/browser exec -- playwright install firefox webkit
RUI_CLIENT_ENGINE=firefox npm --prefix apps/browser run test:live
RUI_CLIENT_ENGINE=webkit npm --prefix apps/browser run test:live
node apps/browser/tests/sites.mjs https://en.wikipedia.org/wiki/Web_browser https://www.youtube.com/watch?v=aqz-KE-bpKQ
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 conformance/check.py
```

Playwright's WebKit requires its platform system libraries. `RUI_CLIENT_EXECUTABLE` can point to a compatible client-browser executable/wrapper. The live test source executable can be selected with `RUI_TEST_SOURCE_EXECUTABLE`.
Direct adapter tests use `RUI_CHROMIUM`.
`RUI_TEST_DIRECT_ICE=1` selects direct source ICE addresses in the scoped-media test. Tests use temporary profiles and self-signed local HTTPS; their client certificate bypass is confined to tests. JSON reports and screenshots are written to ignored `test-output/` and explicitly state `physicalDevice: false`.

The proxy test runs the live flow through a local TLS-terminating reverse proxy with a different backend port, preserving Host and Origin as Tailscale Serve does. It also checks rejection of an incorrect Host or Origin and spoofed forwarding headers. The path variant strips `/bart` as Tailscale Serve does and verifies that JS/CSS/WASM, API requests, and WebSockets remain under the mount, including a bare `/bart` entry and remembered-device reload. It also checks that an existing root-path credential is untouched. This tests proxy compatibility; it does not change the machine’s Tailscale configuration.

The worker fixture checks Unicode edits, stale source and navigation rejection, password redaction/single-use/expiry/lease binding, upload path confinement, native form controls, embedded frames, dialogs, downloads and tabs. The live test exercises production assets through the Rust host, HTTPS/WSS, local approval, control, navigation, edits, password entry, release, remembered-device reconnect, host restart with a persistent login cookie, and host revocation.

The document acceptance checks cover the following cases.

| Case | Verified behavior |
| --- | --- |
| Responsive page | Native text, CSS grid, pseudo-elements, SVG, decoded images and fonts |
| Forms and editing | Unicode values, Enter submission, select controls, simple rich-text structure, stale edit rejection |
| Dynamic page | DOM patches, stylesheet replacement, virtualized scrolling, navigation generation fences |
| Embedded content | Open shadow trees and cross-origin frame rendering and actions |
| Canvas and WebGL | Element streams, visible screenshot pixels, bounded canvas drag input |
| Video and audio | Decoded video and inbound audio packets in Chromium, Firefox and WebKit |
| Public sites | Wikipedia text/images and YouTube play, audio, pause and seek through the replica |
| Attachment | Existing-browser actions, excess-tab preservation, viewport restoration and non-destructive detach |
| Paired application | Resource integrity rejection/recovery, control, reconnect, restart, revocation and proxy path mounting |

The paired control regression runs with `npm --prefix apps/browser run test:control`.
It checks capture-limit recovery, slow navigation, touch input, keyboard viewport changes, delayed command receipts, and uncertain-action recovery.
Set `RUI_TEST_LATENCY_MS=150` to add delay to each HTTP request.
The public-site test reports search-to-render and video-to-frame times, plus decoded and transferred document bytes.
Use `RUI_TEST_HEADED=1 RUI_TEST_PUBLIC=1 npm --prefix apps/browser run test:control` to include a visible host browser and live YouTube search results, thumbnails, and video playback.
The playback check waits through pre-roll ads and verifies that the selected video continues on the host and phone view.
Add `RUI_TEST_RECOVERY=1` to inject document, media-open, media-renewal, and playback failures into the YouTube flow and check recovery before further navigation and input.
This mode also stalls the host page for seven seconds and checks that control stays connected.
Set `RUI_TEST_RESULTS_SCROLLS=4` to load a large YouTube results page before opening a video.
Set `RUI_TEST_VIDEO_TARGET=thumbnail` to open its thumbnail; the default opens its title.
Set `RUI_TEST_BLOCK_ICE=1` for the media, paired control, or phone-mode test to verify that playback does not need direct WebRTC.
Set `RUI_TEST_PAUSE=1` for the public-site control test to check repeated phone taps that pause and resume the host video.
The media test checks decoded audio, sound activation, failed-read recovery and new frame colors after a source change.
These checks simulate a phone viewport; they do not replace testing on a physical phone.

The phone-mode test starts a temporary real Tailscale Serve route and checks HTTPS without a certificate bypass.
It requires a signed-in local Tailscale daemon and removes the temporary route after the test.
The public-site probe requires network access and current site availability.
It saves screenshots and reports actual results; it does not replace deterministic fixtures.
No protected-media or physical-device pass is claimed.

Physical phone acceptance: pair through a trusted HTTPS QR invitation; navigate and switch tabs; complete a standard login form; edit with the OS keyboard and IME; upload a host file and download to the host; background/foreground and disconnect/reconnect; change the same field locally on the host while drafting; revoke the phone on the host and confirm it can no longer act. Repeat on iOS Safari and Android Chrome before calling mobile support validated.

## Implementation

`run.py` owns the process group and profile lock. `rui-browser-host` owns authentication, control leases, the command ledger, source validation and RUI publications. The private Node worker owns Playwright/CDP and serves the static mobile app plus the same-origin WSS proxy. Private mode-0600 Unix sockets connect the worker and Rust host; the proxy verifies the internal RUI TLS identity. Public APIs expose pairing and password staging, never CDP, selectors or arbitrary script execution.

The negotiated `browser.semantic.v1` profile adds optional input-type metadata and a reconnect command-sequence floor. Browser actions terminate with `Dispatched`; publication does not falsely claim an atomic DOM snapshot or causal completion. Existing GTK/vector profiles retain their existing command completion behavior.

Clean browser observations are reused for up to one second.
Accessibility and page lifecycle events invalidate this cache.
Document and navigation commands validate the selected page and source generation without a full accessibility crawl.
Other semantic commands refresh their source state before dispatch.


## Document transport

The optional `browser.document.v1` extension uses authenticated HTTP document checkpoints, node deltas and content-addressed resources.
It reuses Rust's active-view authorization before and after capture or resource delivery.
It does not add a DOM profile to the core RUI binary wire format.
Input passes through the existing Rust lease and command ledger before the browser adapter resolves document generation and node identity.
Media signaling uses the same view grant and binds each stream to its source document and approved client.
The iframe blocks page scripts with CSP, strips executable attributes and blocks remote resource loads.
The host fetches source assets with at most eight concurrent requests.
The client downloads up to four resource batches at once, with up to 32 small resources per batch.
Each resource keeps its content hash and size checks.
Unchanged source styles use version references, and the receiver retains parsed styles and verified resources.
Large JSON responses use gzip when the client supports it.
Its sandbox permits native canvas/media rendering; the strict script CSP remains active.
Website scripts are not copied to the client.

Performance diagnostics are available as `window.ruiPerformance` in the phone browser console.
Counters include document requests, changed nodes, render time, encoded media bytes, frame callbacks, buffer depth, dropped frames, and reconnects.
Run `cd apps/browser && node tests/performance.mjs` for idle, live DOM, canvas, and video fixtures.
The report includes browser task time, JavaScript heap size, payload throughput before HTTP compression, and action-to-display delay.
These synthetic measurements do not include GPU process usage or physical phone battery use.

Document capture tracks changed subtrees with mutation and input events.
History API and fragment URL changes keep the current document view, so pages that update their URL while scrolling do not reload on the phone.
Only a replaced source document, or a route change after half of the node identity budget is used, starts a new document view.
Idle document requests wait for a change or a bounded heartbeat.
Active update intervals follow capture cost, from 50 to 300 ms.
A full capture every two seconds checks properties that do not emit mutation events.
The phone preserves unchanged records and skips unchanged DOM subtrees after validating each patch.
Documents with shadow trees or embedded frames keep the full reconciliation path.
Media sessions and field values are maintained even when the surrounding DOM is unchanged.

Detaching waits for page cleanup before disconnecting from an external browser.
Cleanup removes injected observers, listeners, media sessions, and frame state.
Pending captures cannot publish or recreate state after detach.
