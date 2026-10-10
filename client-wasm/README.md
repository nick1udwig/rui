# Rust/WASM client bindings

The existing `rui-protocol-core` builds unchanged for `wasm32-unknown-unknown`.
`rui-client-wasm` adds a small `wasm-bindgen` wrapper around its actual `ViewReceiver`, producing browser ES modules and generated TypeScript declarations.
The same module executes successfully in Node and Chromium. There is no TypeScript reducer or native GTK/Cairo dependency in this module.

The original low-level `Receiver` remains available for conformance and trusted integrations. The `Session` binding now uses the shared `rui-client-core` actor and `rui-transport` codec for binary session negotiation, chunked publications, resources, control, semantic commands and retained recovery. Native networking is gated behind the transport's default `native` feature; the WASM dependency builds its codec without Tokio, TLS, GTK or Cairo.

The [browser library](../client-browser/README.md) supplies WebSocket I/O, rendering, presentation and DOM interaction. The [Vite playground](../demos/web/README.md) consumes that library. Both construction of the low-level receiver and authentication of the supplied session transport remain integration responsibilities.

## Reproduce

From the repository root, with the existing Rust/Python/protoc environment and Node/npm available:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
python3 client-wasm/build.py
npm --prefix client-wasm ci
npm --prefix client-wasm exec -- playwright install chromium
npm --prefix client-wasm test
```

The CLI version must match the pinned Rust dependency. `build.py` checks this before generating bindings.
Set `WASM_BINDGEN` or pass `--wasm-bindgen /path/to/wasm-bindgen` to use a separately installed CLI.
Set `RUI_WASM_CHROMIUM=/path/to/chromium` to use an existing browser executable instead of Playwright's downloaded browser.
The browser test opens an owned headless browser and a temporary loopback HTTP server.
Generated `pkg/`, compiled test files and `node_modules/` are ignored; Rust dependencies and npm tooling are locked.

The `web` output loads through an ES module and explicit asynchronous initialization, making it suitable for a future Vite integration without requiring a WASM bundler plugin.
Node verification uses the same generated ES module with synchronous initialization from the module bytes.

## Verified behavior

The TypeScript consumer is compiled in strict mode against the generated declarations, then runs identical checks in both runtimes:

- The existing eleven-prefix semantic golden trace matches native Rust admission, committed revisions, record IDs and record versions at every prefix, including rejected operations and an exact duplicate.
- Snapshot and transaction revisions, record IDs and versions above `Number.MAX_SAFE_INTEGER` remain exact through `bigint` and `BigUint64Array`.
- Unicode text, including supplementary characters and combining marks, survives input and output copies.
- Bound Envelope decoding rejects malformed input and foreign session bindings without changing committed state.
- Missing vector resources keep the previous state installed until their bounded chunks complete and their descriptor-bound SHA-256 digest validates. Unsolicited and corrupt resources reject.
- Disconnect retains committed state while rejecting reception; same-binding reconnection restores reception. Close releases Rust reservations and cannot be reversed.
- Invalid binding lengths and an insufficient memory ceiling fail through JavaScript exceptions.

The current release includes the session actor and TypeScript serialization as well as the receiver. `build.py` prints its actual uncompressed/gzipped size and copies generated browser bindings into `client-browser/wasm/`. The original receiver-only feasibility artifact was 602,362 bytes (150,813 gzipped); the current module is approximately 840 kB (215 kB gzipped). No `wasm-opt` pass is required.

The bridge copies incoming byte arrays into WASM and copies returned state into JavaScript. IDs and clocks remain `bigint`; the public browser API validates unsigned 64-bit inputs before calling the generated ABI. Rust's ledger covers receiver/session-owned allocations, not caller-retained JavaScript copies, browser socket buffers or allocator capacity. The browser library documents its separate ownership, rendering and presentation bounds.
