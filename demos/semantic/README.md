# Semantic browser presentation

This separate consumer uses `BrowserClient` in semantic-only mode and the reusable
`SemanticView` mapping. It constructs no canvas and sends no Presented. The current
UI is read-only: local navigation and text selection do not invoke source focus,
scroll, resize or edits. Guarded DOM controls are the next S3 increment.

```sh
python3 demos/web/run.py --presentation semantic --application editor
# Use --application corpus for the first owned source.
```

The shared launcher builds the real WASM module and TypeScript library, installs
locked dependencies and starts the selected owned GTK source plus this Vite consumer.
Use `--skip-build` after a successful build. HTTPS/certificate, public-host and exact
Origin options match the mirror launcher. HTTP/WS is explicitly enabled only by the
local development page; credentials remain a caller-selected file. A remote/tailnet
workflow still needs the L5 two-machine acceptance evidence.

Mapped surfaces supply semantic roots and modal relationships. The mapping follows
ordered semantic children, explicit reading order and already materialized semantic
groups. It never uses Scene/Binding coordinates, GTK classes, labels or known IDs to
choose control roles. Root/Group become labeled sections; text fields use read-only
native textareas, Boolean states use disabled native checkboxes, lists/items retain
list semantics, and buttons/custom canvases remain focusable information. Unsupported
actions/schemas are described, not dispatched. Unmapped surfaces are omitted.

Labels/descriptions/values/text use textContent or value. LabelledBy relations resolve
only within the authorized materialized view; description relationships and native
availability/state/completeness metadata remain explicit. Available, unavailable and
redacted text differ. Partial collections display materialized count, known total or
unknown total and source selection; an unmaterialized list is not an empty full list.

Keys are protocol bigint identities. Harmless updates preserve elements and local
focus/selection; removals, disconnect/reset and disposal purge those identities and
content. Limits default to 1024 semantic nodes, depth 64, 512 KiB text, at most 8192
input records and 16384 reference/action update units. Exceeding a local limit clears
the view and reports an unavailable display instead of inventing completeness. Limits
may be lowered; maximum supported nodes/depth/text are 4096/128/1 MiB. Each view has
its own DOM allocation responsibility, separate from Rust/WASM and Canvas budgets.

The build includes runtime WASM and has no DOM construction on module import.
Three-browser tests use exact source-derived corpus/editor states and verify string
escaping, new identities, arbitrary geometry, redaction, partial collections, stable
DOM/focus, unmapped cleanup and limits. Live owned GTK checks call render before
SemanticReady, then independently replay the exact authorized packets. Responsive
layout/accessibility/device acceptance remains S4 work; viewport emulation is not a
mobile-browser claim.
