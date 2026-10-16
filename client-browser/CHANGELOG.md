# Changelog

## 0.1.2 — browser workspace support

- Add negotiated `browser.semantic.v1`, optional browser input hints and guarded `setBrowserValue` for browser producers.
- Treat confirmed browser dispatch as terminal only under that profile; existing GTK and native command semantics are unchanged.


## 0.1.1 — additive ownership diagnostics

- Expose conservative Rust `memoryPeak` separately from JS/socket/allocator capacity,
  for cold/recovery/lifecycle measurements. No negotiated limit changes.
- Keep matching crate/package versions and regenerate the binding/API baseline.
- Make disconnect after final disposal idempotent without accessing freed WASM;
  verify caller-owned page departure/restoration through the installed consumer.

## 0.1.0 — private experimental distribution

- Self-contained ESM/declaration/WASM tarball and explicit asynchronous initialization.
- Fresh scoped control-request namespaces; safe disposed status and bounded WASM ABI preflight.
- Shared Rust native/browser replica, bounded resources and exact command recovery.
- Mirror Canvas rendering and semantic-only DOM/control adapters, with typed bigint state.
- Clean Vite consumer with production assets under `/apps/rui/`, compatibility checks,
  initialization/MIME/CSP failures and replaceable transport/renderer acceptance.

No public registry release or project license is implied. During 0.x, a breaking
JavaScript API, asset path, generated ABI or supported-profile change requires a minor
version change and migration notes here; patch versions preserve those contracts.
Additive changes may use a patch version only when existing consumers still work.
Rust crates and the browser package use the same version. Wire negotiation, generated
WASM/JS binding compatibility and package API compatibility are separate checks.
Always deploy the matching JS/declarations/WASM set atomically; do not mix releases.
