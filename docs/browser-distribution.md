# Browser distribution and support

The private `@rui/client-browser` 0.1.1 tarball contains runtime ESM, TypeScript
declarations, generated WASM/JS, compatibility metadata, README, changelog and
third-party notices. It has no install hook, runtime npm dependencies, fixture data,
credentials, source, Rust toolchain or GTK dependency. A browser-only client needs
those installed assets and a caller-provisioned approved identity/credential.

Build WASM with the exact locked CLI, then pack:

```sh
python3 client-wasm/build.py
npm --prefix client-browser ci
(cd client-browser && npm pack --pack-destination /tmp)
```

Install the tarball from a separate application's directory. Its declared ESM entry
point exports the client, transport types/default adapter, Canvas renderer, complete
model, timing helpers and semantic view/controls. Module import is safe without DOM;
creating/rendering clients requires a browser. Node is a build/inspection runtime,
not a promised headless client platform.

| Component | Tested contract |
| --- | --- |
| Tooling | Node 26.8.1, TypeScript 5.9.3, Vite 8.3.4 |
| Rust/bindings | Rust 1.98.1, wasm32-unknown-unknown, wasm-bindgen 0.2.129 |
| Desktop assets | Chromium 153.0.8010.12, Firefox 155, Playwright WebKit 26.6 |
| Live owned source | system Chromium 151.0.7922.173, GTK 4.22.4, Cairo 1.18.4 |
| Wire | RUI/0.1; explicit negotiated mirror or semantic-only profile subset |
| Package/crates | matching 0.1.1; generated JS/WASM/declarations deployed together |
| Physical Safari/mobile/screen readers | acceptance not measured |
| Two-machine tailnet | documented workflow; acceptance not measured |

`compatibility.json` binds asset hashes, declaration hashes, crate/package versions,
binding-generator version and both wire-schema hashes. `scripts/api-baseline.json`
freezes public exports/declarations and generated WASM imports/exports. Normal builds
reject drift; after an intentional reviewed contract change, update version and
changelog, then run `npm --prefix client-browser run compatibility:update`. The
pre-1.0 policy is in the package changelog. Wire negotiation does not make mismatched
binding binaries compatible. Cache content-hashed assets immutably and deploy an
entire version atomically.

Vite copies the WASM referenced by the generated module's `new URL` and respects
`base`, demonstrated under `/apps/rui/`. Other bundlers must emit that dependency or
copy the exported `@rui/client-browser/wasm` asset and call `initialize(absoluteUrl)`
before `BrowserClient.create`. The default initialization is asynchronous, shared
and retryable after failure. An initialized module instance rejects a different URL. Preflight bounds the startup
asset to 8 MiB and 1024 delivered chunks, validates a browser module and required
session exports before initializing the generated binding. Thus a valid foreign
WASM binary fails without poisoning the binding's singleton. This startup bound is
separate from negotiated wire/resource budgets.
Do not point initialization at generated JS or an ABI from another release.

Serve HTML/JS with their normal MIME types and WASM as `application/wasm`. A wrong
WASM MIME uses buffered fallback and warns; 404, corrupt bytes, blocked fetch or CSP
compilation denial reject initialization. A minimal tested CSP has
`script-src 'self' 'wasm-unsafe-eval'` and `connect-src` allowing the asset origin and
the exact WSS endpoint. No cross-origin isolation, shared memory or inline script is
required. Application styles and images need their own explicit directives.

For direct WSS, provision a host certificate trusted by machine B's browser and
configure the host's exact browser Origin (scheme, hostname and port). Browser TLS
verification is authoritative; arbitrary native DER pinning is not a browser API.
For a proxy, use trusted HTTPS on its front and retain certificate verification and
the expected TLS hostname on its backend. The example in `demos/web/vite.config.ts`
uses a CA-configured HTTPS Agent with `rejectUnauthorized: true`, forwards binary
frames and the original Origin, and never serves the credential or private keys.
Vite preview is the local acceptance server, not a production deployment service.

Credentials are provided asynchronously by the caller; keep them outside static
roots, bundles, URLs and automatic browser storage. The library clears its transient
copies. It obtains a fresh request namespace with crypto.getRandomValues for each
new actor; the lower-level Rust/WASM Session API requires callers to provide one
before connection. Reconnect preserves it and its monotonic request counter. Custom transports synchronously admit/copy ordered binary bytes, report
bounded buffering, deliver asynchronous callbacks and own/clear admitted copies.
Custom renderers receive owned committed state/verified resource copies and report
no readiness themselves: the client waits for an actual visible frame opportunity.
Status getters after disposal return empty capability/state, zero pending/owned
memory and false resource readiness without accessing freed WASM. These are ownership
status values, not measurements of the browser allocator's retained capacity.

```sh
# Three-engine installed-artifact gate, no GTK required:
npm --prefix client-browser run test:package
# Actual owned GTK source, production HTTPS/WSS and both adapter seams:
python3 client-browser/tests/package-live.py --xvfb /path/to/Xvfb
```

The clean fixture is copied into a fresh directory outside the checkout, installs
the actual tarball, compiles against installed declarations and builds production
assets under a non-root base. Portable CI checks import without DOM, exact tarball
contents, initialization/retry/concurrency, MIME/CSP and failure/disposal. The pinned
GTK CI job exercises observation, explicit control, guarded button/Unicode edits,
source-side edits, release, reconnect without authority and disposal through default
and replacement transport/renderer APIs. Both jobs test distributed files rather
than repository-relative imports.

The live local fixture uses scoped trust exceptions for owned test certificate keys;
it does not establish public-CA or physical-device trust. Follow the two-machine
instructions in `demos/web/README.md` with machine A serving the host and machine B
running only the browser. Record hostname and IP cases, exact Origins, trusted HTTPS,
asset loading and both edit directions in a separate report before claiming that
acceptance boundary. Local `0.0.0.0` binding plus loopback is not that evidence.

The repository has no project license. The package remains private and `UNLICENSED`.
`scripts/notices.py` retains licenses/notices for the locked WASM build closure
(including build/macros conservatively) and the compiler's complete standard-library
notices. Runtime tarballs exclude npm tooling, GTK and fixture fonts. Registry
publication and project license selection are separate release actions.

An approved client instance ID identifies one command producer for a host epoch.
Use `disconnect`/`connect` on the same actor to preserve its sequence and exact pending
bytes. Final `dispose` discards that ledger; a new mutating actor needs a separately
approved client instance ID or a new host epoch. Reusing a disposed producer identity
can observe/request control, but its reset command sequence is rejected by the host.
The library does not infer a sequence or recreate an old ambiguous operation.
