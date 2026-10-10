# Browser client library

`@rui/client-browser` is an experimental TypeScript library backed by the existing Rust receiver and the portable `rui-client-core` session actor through WASM. The [Vite playground](../demos/web/README.md) consumes its public API; the library contains no demo labels, widget IDs or application rules.

The boundaries are explicit:

- Rust handles binary schema checks, negotiated limits, session/view/epoch bindings, exact publication and resource hashes, atomic snapshot/transaction application, leases, semantic commands and pending command bytes.
- `BinaryTransport` carries ordered binary frames. The default browser adapter uses `rui.v1` WebSockets with browser TLS verification. `wss:` is required unless the caller explicitly enables development `ws:`.
- `Renderer` consumes committed records and verified resource bytes. The included `CanvasRenderer` supports all nine primitives and negotiated single-outline glyph runs.
- The pointer adapter delegates hit testing to Rust. Editing uses an opaque captured context stamp; changed authority, focus, selection, revision or text clocks reject a stale draft.

Build the WASM module first with `python3 client-wasm/build.py`, then run these from the repository root:

```sh
npm --prefix client-browser ci
npm --prefix client-browser run build
```

The package exports compiled ES modules and declarations from `dist/`, with generated bindings in `wasm/`. Build and install a self-contained private tarball:

```sh
(cd client-browser && npm pack --pack-destination /tmp)
# In a separate application:
npm install /tmp/rui-client-browser-0.1.0.tgz
```

`npm pack` checks the API/declarations/generated ABI and includes only runtime files,
compatibility metadata and notices. It excludes tests, fixtures, source and tooling.
Vite resolves the generated binding's `new URL(..., import.meta.url)` WASM asset,
including a configured non-root `base`. Other bundlers must preserve that asset
relationship, or call exported `initialize(absoluteWasmUrl)` before creating clients. Startup asset preflight is bounded to 8 MiB/1024 delivered
chunks and requires the expected browser session exports.
`BrowserClient.create` initializes the bundled module automatically. Failed
initialization (including a valid foreign ABI) rejects and can be retried; one module instance cannot switch a loaded
ABI to another URL. Module import in Node does not construct DOM or initialize WASM.

Serve JS as JavaScript and WASM as `application/wasm`. Incorrect WASM MIME can use the
buffered initialization, with a warning; 404/corrupt assets and CSP rejection fail.
A minimal CSP needs `script-src 'self' 'wasm-unsafe-eval'` and `connect-src` permitting
the WASM origin and exact WSS host. Restrict other directives for your application.
Keep credential files outside the static serving root and use caller-owned provisioning.

Tested tooling is Node 26.8.1, TypeScript 5.9.3, Vite 8.3.4 and Rust 1.98.1 with
wasm-bindgen 0.2.129. Desktop Chromium 153.0.8010.12, Firefox 155 and Playwright WebKit
26.6 pass the distributed asset gate; system Chromium 151.0.7922.173 passes the live
owned GTK gate. These are tested versions, not minimum browser-version promises.
Physical Safari/mobile/screen-reader and two-machine tailnet acceptance remain
unverified. See [distribution checks](../docs/browser-distribution.md).

The repository has no declared project license. This package stays `private: true`
and `UNLICENSED`; third-party notices preserve the locked WASM build closure and
standard-library notices without assigning a license to RUI. Registry publishing
and a project licensing decision are separate release actions. See [changelog](CHANGELOG.md).

```ts
import { BrowserClient, bindPointer } from '@rui/client-browser';

const client = await BrowserClient.create({
  url: 'wss://host.example/rui',
  clientId: approvedClientId,
  credential: async () => new Uint8Array(await credentialFile.arrayBuffer()),
  selection: 'fixture',
  canvas,
  onEvent: event => updateStatus(event),
});
await client.connect();
client.requestControl();
const unbind = bindPointer(client, canvas, target => openEditor(target), console.error);
// After host focus and presentation:
const captured = client.captureEdit(target);
client.commitEdit(captured, draft);
// To retain committed state across a socket replacement:
client.disconnect();
await client.connect();
// Final teardown:
unbind();
client.dispose();
```

The credential provider belongs to the caller. Credentials never enter a URL, browser storage or the bundle. `client.disconnect()` clears JavaScript state and renderer buffers while Rust retains its guarded committed state and unresolved commands; `dispose()` releases the Rust receiver, event listeners, timers, canvas and renderer caches. Caller-retained state copies remain caller-owned.

An ambiguous command is queried after reconnect. `retryPending()` explicitly resends its original bytes, including the original sequence and lease; it never recreates an edit under new authority. A new epoch retires old pending commands. No automatic reconnect or automatic new control grant is inferred.

IDs, revisions, versions and scalar indices use `bigint`. Public command methods validate the unsigned 64-bit range before calling the generated ABI. `scalarOffset()` converts DOM UTF-16 boundaries to Unicode scalar offsets and rejects split surrogate pairs. Selection commands are explicit; replacing a draft uses the full authoritative text range. The initial pointer adapter supports semantic activation and focus. Raw keyboard/pointer batches, source IME composition, fingerprint-font shaping, cursors and session-shell controls are not advertised by this browser session.

## Rendering and presentation contract

`renderer.browser-canvas-coverage-linear.v1` uses Canvas for vector coverage. Paint is composited separately in premultiplied linear sRGB, including isolated group/surface opacity and premultiplied gradient interpolation. The final transfer uses a 16-bit lookup with less than one output-byte error. No DOM fonts, text reshaping, screenshots or transmitted raster resources are used. Local coverage masks and output pixels are renderer-owned diagnostic/display data.

Browser coverage can differ from the pinned native Cairo reference; this is a separate experimental renderer profile, not a claim of bit-identical Cairo output. Glyphs use the host's verified outline, size and position. Unsupported fonts or incomplete mapped surfaces fail explicitly.

Custom renderers should implement `reset()` to clear scope-bound pixels and caches on reset or disconnect. `CanvasRenderer` reuses its linear/output buffers and geometry coverage, with full repaint each frame. Color-only changes reuse coverage; all transforms, clips, path identities and stroke geometry remain in the cache key. Its fourth constructor argument `false` selects the allocation/paint reference path for pixel comparisons. `allocationStats()` reports bounded owned references; browser allocator capacity and native backend allocations remain separate.

The renderer bounds its viewport to 4,194,304 pixels, work to 67,108,864 pixel visits per frame, depth to 128, path bodies to 4 MiB/65,536 commands and working buffers plus caches to 64 MiB. Rust's negotiated ledger is separate from JavaScript copies, native browser socket buffers and allocator heap capacity. The default socket adapter rejects messages above the negotiated ceiling before the WASM copy and bounds outgoing buffered bytes; the browser may allocate an incoming WebSocket frame before delivering it to JavaScript.

`Applied` means the Rust receiver committed verified state. Drawing does not immediately send `Presented`. A following visible animation-frame callback supplies a presentation opportunity for the previous canvas submission, with revision and connection checks. This acknowledges an opportunity to paint, not physical scanout. Hidden tabs cancel pending presentation, release control and keep receiving state; returning visible redraws and requires a fresh presentation opportunity. Control must then be requested again. Changing a viewport also pauses input until its new frame is presented.

## Checks

```sh
npm --prefix client-browser exec -- playwright install chromium
npm --prefix client-browser test
cargo test --locked -p rui-client-core
```

Set `RUI_WASM_CHROMIUM` to use an existing Chromium executable. Pixel tests cover all nine primitives, linear source-over and gradients, isolated opacity, clips, host-shaped glyphs, Unicode offsets, session replacement, limits and disposal. The [live integration check](../demos/web/README.md) also uses an owned GTK host and private Xvfb display.

An approved client instance ID identifies one command producer for a host epoch.
Use `disconnect`/`connect` on the same actor to preserve its sequence and exact pending
bytes. Final `dispose` discards that ledger; a new mutating actor needs a separately
approved client instance ID or a new host epoch. Reusing a disposed producer identity
can observe/request control, but its reset command sequence is rejected by the host.
The library does not infer a sequence or recreate an old ambiguous operation.
