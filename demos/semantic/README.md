# Semantic browser presentation

This separate consumer uses `BrowserClient` in semantic-only mode and the reusable
`SemanticView` mapping with optional `SemanticControls`. It constructs no canvas
and sends no Presented. Observers can navigate and select local text. Controllers
explicitly activate source controls, select stable items, change declared Boolean
values, or request source focus before editing a local draft.

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
native textareas, Boolean states use native checkboxes (disabled for observers), lists/items retain
list semantics, and buttons/custom canvases retain focusable information and advertised actions. Unsupported
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

The interaction adapter uses only public library APIs. Local DOM focus, selection,
input, paste, deletion and composition do not dispatch source commands. Edit draft
requests source focus, waits for confirmed focus/readiness, and captures the original
target, revision, text/selection/focus clocks and lease token. Apply draft (or
Ctrl+Enter after local composition ends) submits that bounded committed string. The
16 KiB UTF-8 ceiling is enforced before WASM copying. Source text remains visible
separately; a newer publication never silently rebases a draft. Conflicts preserve
the local draft until an explicit new edit or authority/scope cleanup.

Pending and rejected outcomes appear in a live status. Unknown/OutcomeUnknown/
OutcomeExpired block another command; resolving an original command remains an
explicit caller operation, never a freshly created retry. Revocation, removal,
reset, disconnect and disposal retire the draft. Reconnect does not request control.
Only schema-1 advertised actions are enabled, with native host guards rechecked.

Fresh owned-display DOM checks on both applications:

```sh
python3 demos/semantic/session_check.py --dom-controls --xvfb /path/to/Xvfb --output /tmp/rui-semantic-dom-corpus
python3 demos/semantic/session_check.py --dom-controls --application editor --xvfb /path/to/Xvfb --output /tmp/rui-semantic-dom-editor
```

[Retained authorized traces](../../fixtures/gtk/semantic-controls-golden) include
independent Python replay, the three intended effects, editor Boolean mutation,
local-composition silence, stale-selection draft rejection, revocation and reconnect.
Three-engine DOM regressions additionally exercise unknown outcomes, exact UTF-8
limits and zero mutation from observer navigation. Source-side guard faults remain
covered by the native application/context tests. S4 layout/device and S5 simultaneous
presentation acceptance remain outstanding.

Local layout uses semantic groups and reading order: wide groups use an adaptive
grid, narrow groups/forms stack, collections retain ordered list structure, and
published mapped modal relationships produce named dialogs. Source modality is
announced but does not trap observers or mark their whole local page modal. Labels
and text use automatic direction for bidirectional content. Controls have source
names/descriptions and visible keyboard focus; action buttons include the source
name, pending/error status is a polite live region, and source focus has a distinct
start border. No local resize/scroll/touch gesture is translated to raw source input.

[Three-engine layout evidence](../../fixtures/browser/semantic-layout) covers both
source-derived applications at 320/800/1440 px, long Unicode/bidirectional labels,
200% text, partial collections, dialogs, keyboard traversal/Enter activation,
unchanged source data and zero commands from observer reflow/navigation. Chromium's
actual accessibility API verifies text names and status roles; all engines verify
accessible snapshots. Narrow/wide screenshots were visually reviewed. These are
automated accessibility checks, not a physical screen-reader usability session.
Physical screen-reader and actual mobile-device acceptance remain unverified;
Playwright viewport/engine evidence does not establish Safari/iOS/Android support.

The S5 presentation check provisions separate approved identities/credentials and
connects a semantic DOM controller plus an actual Canvas mirror observer to the
same owned host. It discovers targets from semantic ownership/roles/actions, swaps
the lease and runs the button/Unicode/stable-item effects from both clients. It
checks delayed semantic delivery while the mirror progresses, mid-draft release,
local/native geometry changes, reset/reconnect without authority restoration,
source replacement with retired targets, and permission reduction to a read-only
credential. The editor also transmits a two-of-ten partial collection and exercises
a real source modal plus retired/rematerialized text control. Both exact authorized
projections are independently replayed by Python at every publication prefix.

```sh
python3 demos/semantic/presentation_check.py --xvfb /path/to/Xvfb --output /tmp/rui-presentation-corpus
python3 demos/semantic/presentation_check.py --application editor --xvfb /path/to/Xvfb --output /tmp/rui-presentation-editor
```

[Retained boundary evidence](../../fixtures/gtk/presentation-boundary) compares
authorized source values and each client's full projection, rather than assuming
private fields or visual records are identical. Semantic traffic has no Scene,
Binding, glyph-resource download, Presented, raw/composition/resize command. The
mirror reports actual presentation opportunities. Unicode visual acceptance uses
source outline fonts; [a fresh color-glyph negative](../../fixtures/browser/semantic-color-glyph-negative.json)
records emoji rejection as incomplete visuals, with no raster fallback. This scope
covers the two cooperating applications and one writer, not arbitrary applications
or concurrent collaboration. Device and external two-machine checks remain separate.
