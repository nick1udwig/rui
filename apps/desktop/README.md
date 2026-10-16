# Existing desktop applications

RUI Desktop attaches to applications already running in your Linux login session.
The browser client shows their accessible content as responsive controls. It does
not stream pixels or reproduce the desktop's layout. No application modification
or RUI-specific launch is required, provided the application exposes AT-SPI.

## Run

Run this from a terminal in the desktop session, as the desktop user:

```sh
python3 apps/desktop/run.py \
  --public-url https://foo.tailnet.ts.net/desktop \
  --port 8445
```

In another terminal, configure the same mount:

```sh
tailscale serve --bg --https=443 --set-path=/desktop http://127.0.0.1:8445
```

The launcher builds the existing Rust host, WASM client and web assets. Subsequent
runs can add `--skip-build`. All browser launcher HTTPS/listen/public-path options
also work here; `--http-local` is available for local development. The desktop
mode does not launch Chromium. Use a different port and path to run it alongside
the browser workspace.

System dependencies: Python 3 with PyGObject/Gio, the AT-SPI accessibility bus and
registry, plus the browser workspace's Rust, Node/npm and OpenSSL build tools.
The agent discovers the accessibility bus through `org.a11y.Bus.GetAddress`, or
uses an explicitly supplied `AT_SPI_BUS_ADDRESS`. It does not change desktop
settings or force accessibility into applications that have it disabled.

For a local graphical picker, run this in another desktop terminal after the host
starts (use the same `--state-dir` if you customized it):

```sh
python3 apps/desktop/share.py
```

Choose **Share**, **Stop sharing**, or **Check support**. Checking support reads
local capability counts without sharing the app. Closed shared apps remain listed
so their approval can be removed. The picker uses the private Unix socket, has no
HTTP endpoint, and its own controls are excluded from sharing, including after a
host restart. It requires GTK4/PyGObject. Device pairing approval remains in the
host terminal.

For scripts, `share.py --list` prints local app metadata and
`share.py --diagnose APP_ID` prints counts. These options do not require GTK.

In the host terminal:

```text
apps
share APP_ID
unshare APP_ID
diagnose APP_ID
```

`apps` lists accessible application names, PIDs and opaque IDs locally. Copy an ID
into `share`. That authorizes that running application, including its accessible
windows and dialogs, for paired devices. Up to eight applications can be shared;
the mobile app switcher chooses one at a time. Its window picker shows one
accessible window at a time. A modal dialog takes precedence and returns to the
previous window when dismissed; closing the selected window chooses a survivor.
Switching the phone view does not
raise or focus a host window. Sharing a new app selects it immediately.

Scan the invitation, then type `approve CODE` on the host. Choose **Take control**
on the phone to edit or activate controls. **Apply** replaces the complete text
field; **Save document** (or the application's equivalent) invokes the actual
app's action. Remote edits do not automatically save documents.

Device approvals are remembered in `~/.local/state/rui/desktop`; application
approvals last only for the host process. `revoke CLIENT_ID` withdraws a device's
access. `unshare APP_ID` removes that application's content and actions. Quitting
RUI leaves attached applications running. An app restart needs a new `apps` /
`share` approval, even if the name and PID are reused.

## Current scope

- Existing app discovery and explicit local approval; no arbitrary client PID,
  D-Bus destination, native handle, script, or shell command.
- Accessible text, bounded full-field edits, default actions on buttons, menu
  items, tabs, links, and related actionable roles. Checkbox/radio defaults appear
  as buttons with their observed checked state.
- Explicit single/multiple selection and deselection for exposed list items and
  tabs. Numeric fields bind to the app's Value interface, validate finite values
  against observed bounds, and expose scrollbar movement as bounded increments.
  Reordering a list or changing a value invalidates old commands.
- Phone-sized touch UI, host-authoritative values, preserved conflicting drafts,
  reconnect, device revocation, and app switching.
- Password-role objects are redacted without querying their text or descendants.
  All-mask text is also treated as protected because some GTK entries omit the
  password role. Password entry is unavailable. This relies on applications
  exposing correct accessibility information; custom secret widgets cannot be
  identified universally.
- Controls with no usable accessible name are unavailable. Source-advertised
  scroll-to actions are identified as scrolling; they do not claim to open files.
- Read-only presentation for unsupported fields/actions. No screenshots, graphics,
  canvas, video, clipboard, keyboard shortcuts, pointer injection, terminal input,
  compositor control, application launching, or guaranteed arbitrary-app coverage.
- Single shared selected app and one controlling device, as in the browser host.
  All paired viewers see the shared selection.

An app missing from `apps` may not expose accessibility or may be running in
another login session. An unavailable app may have exited, stalled, exceeded the
observation limits, or changed during the read. The client clears unavailable
content and retries. It never redirects an old approval to a replacement app.

## Design and limits

The existing `rui-browser-host` owns pairing, authorization, leases and the exact
retry ledger. `apps/browser/worker.mjs` serves the same client and delegates to
`DesktopWorkspace` in desktop mode. The wire selection is `desktop`; the existing
`browser.semantic.v1` extension is reused for full text values and observed
`Dispatched` results. That historical profile name does not imply DOM/CDP access.
No wire format or client library API changes are required.

Only the host's private worker socket accepts app discovery/share commands.
Native D-Bus references remain on the owner lane. A source identity binds the
accessibility connection, root, Unix PID, process start time and same-user UID.
Public IDs are retired on unshare, observation loss, or selection replacement.

Blocking D-Bus work runs in disposable Python processes with a 384 MiB address
space limit, 400 ms call timeout, 1.8 second observation deadline, and 2.4 second
owner timeout. Background observations do not block serving the cached client
view. Reads run in batches of up to 32 concurrent D-Bus calls. Limits include
64 discovered apps, eight approved apps, 512 visible native nodes (1,024 visited
nodes), up to 1,024 nodes including generated semantic controls, 128 children per
node, depth 64, 8,192 calls, 16 KiB text fields and 768 KiB native
responses. Limits fail closed, rather than silently truncating editable text.

Before dispatch the worker verifies the source identity and rereads the entire
app. The target fingerprint must match the approved observation, including its text,
capabilities, default action identity, ownership chain and active modal state.
Unrelated status labels may change without invalidating an unchanged control.
Hidden branches are omitted from the mobile view. Modals block other
subtrees; ambiguous multiple modals disable all actions. The post-action read is
returned with the result so following actions use fresh state. A timeout after a
native call is `OutcomeUnknown`, never a safely retryable failure.

AT-SPI has no atomic capture or compare-and-set edit operation. This is **observed,
partial semantics**, with a remaining race between revalidation and application
execution. It does not establish compositor surface identity or intercept local
input. Draft conflict detection and modal checks reduce mistakes but cannot make
unmodified applications transactional.

## Verification

```sh
node apps/desktop/tests/workspace.test.mjs
python3 -m unittest discover -s apps/browser/tests -p '*_test.py'
cargo test --offline --locked -p rui-browser-host
cargo clippy --offline --locked -p rui-browser-host --all-targets -- -D warnings
npm --prefix apps/browser run build

python3 fixtures/atspi/native_check.py --xvfb /path/to/Xvfb \
  --driver apps/desktop/tests/native_driver.py --output /tmp/rui-desktop-native
RUI_DESKTOP_LIVE=1 python3 fixtures/atspi/native_check.py --xvfb /path/to/Xvfb \
  --driver apps/desktop/tests/native_driver.py --output /tmp/rui-desktop-mobile
RUI_DESKTOP_LIVE=1 RUI_CLIENT_ENGINE=webkit \
  python3 fixtures/atspi/native_check.py --xvfb /path/to/Xvfb \
  --driver apps/desktop/tests/native_driver.py --output /tmp/rui-desktop-webkit
```

Tests use an owned authenticated display and private D-Bus without activating
unrelated desktop services. The source uses ordinary GTK widgets and runs before
the RUI host. The browser test covers two existing apps, local sharing, pairing,
mobile editing, saving a real file, modal handling, draft conflicts, mounted-path
hosting, reconnect, unshare, device revoke and attached-app survival on shutdown.
Screenshots go to ignored `test-output/`. `RUI_WEBKIT_EXECUTABLE` can select a local
WebKit compatibility wrapper on distributions outside Playwright's supported set.
The real-desktop smoke test opens a temporary LibreOffice Writer document in a
separate GTK3 profile on the existing Wayland session, before starting RUI. It
verifies the welcome modal, Unicode edits through the mobile browser, the actual
saved file, reconnect, unshare, and that RUI shutdown leaves Writer running:

```sh
# Requires LibreOffice, a running Wayland session, and built RUI assets.
node apps/desktop/tests/existing-desktop.mjs
RUI_CLIENT_ENGINE=webkit node apps/desktop/tests/existing-desktop.mjs
```

It owns and cleans up only its temporary app/profile/document. It does not share
other running apps or change Tailscale configuration; a local TLS proxy exercises
the mounted URL. Set `RUI_WEBKIT_EXECUTABLE` if a compatibility wrapper is needed.
`RUI_REAL_APP=files` runs the same real-desktop test against a separate Nautilus
instance, an owned temporary folder and a private application bus. It verifies
actual file/folder selection, long-filename mobile layout, unavailable unnamed
menu items, reconnect and app
lifetime. It does not attach to an existing personal file-manager process.

Run all checks and retain a JSON report plus per-check logs:

```sh
RUI_WEBKIT_EXECUTABLE=/path/to/optional-webkit-wrapper \
  python3 apps/desktop/tests/check.py --xvfb /path/to/Xvfb --real-desktop
```

Omit `RUI_WEBKIT_EXECUTABLE` on a supported Playwright installation. Omit
`--real-desktop` for owned-display-only checks; `--engine chromium` limits the
browser engine. Reports go to ignored `test-output/checks/`. The suite includes
native guard tests, Chromium/WebKit mobile flows, Rust checks, client builds and
the original browser workspace regression. Real-desktop checks require Writer,
Nautilus and the host's `WAYLAND_DISPLAY`.

### Observed compatibility

| Source on this machine | Verified scope | Remaining gaps |
| --- | --- | --- |
| GTK4 4.22.4 fixture, two independently started apps | Sharing picker, windows, modal routing, Unicode edit/save, single/multiple selection, tabs, numeric bounds, scrolling, named popovers, stale/reordered controls, password redaction, depth limits, revoke/reconnect | A controlled fixture does not establish arbitrary-app coverage |
| LibreOffice Writer 26.2.5, GTK3 backend | Existing Wayland window, welcome dialog, Unicode paragraph replacement and file save, reconnect/unshare, app survival | Large documents, formatting and other Writer workflows have not been established |
| Nautilus 50.2.2, GTK4 backend | Existing Wayland window, file/folder selection, explicit scroll-to controls, reconnect/unshare, app survival | Unnamed menu items are disabled. Search editing and file opening remain unverified; the tested file-row default action scrolls rather than opens |

The real-app and mobile tests use 390×844 touch browser contexts. Physical
iOS/Android devices and a broader third-party app matrix remain to test.
Omawrite did not register on this machine's accessibility bus during probing;
its support is not established.

## Next steps

1. Test physical mobile keyboards, orientation changes and assistive technology.
2. Expand the compatibility matrix and investigate missing toolkit labels/text
   interfaces. Targeted app adapters need explicit source identity and capability
   evidence; unsupported controls must not be guessed.
3. Add bounded paging or window-scoped reads for applications beyond the current
   observation limits, then test large documents and collections.
4. Consider optional application launching and compositor focus/window management
   as separate capabilities. Existing-session attachment remains the default.

## Implementation sequence

Each step is implemented, tested and corrected before its commit.

1. **Done:** Preserve the verified existing-app foundation, including real Writer coverage.
2. **Done:** Add explicit window navigation, modal routing and closed-window recovery.
3. **Done:** Add observed selection, numeric-value and scrolling controls, with source-side
   revalidation and mobile rendering.
4. **Done:** Add a local sharing picker and per-application compatibility diagnostics.
5. **Done:** Extend native and mobile end-to-end coverage, repeat real-app verification and
   record the supported scope and remaining gaps.

The target remains existing-session semantic interaction on Linux. Pixel streaming,
compositor window management, arbitrary app coverage and transactional semantics for
unmodified apps are not implied by these steps.
