# Status and next steps

RUI has working existing-app, managed-browser, and cooperating GTK paths with different source guarantees.
Use the app guides for current operational behavior, the profile documents for protocol contracts, and the retained acceptance reports for the scope they measured.

## What works today

| Path | Implemented behavior | Coverage boundary |
| --- | --- | --- |
| [Desktop workspace](../apps/desktop/README.md) | Share already-running same-user Linux apps, pair devices, switch apps/windows, and interact through accessible controls | Observed, partial AT-SPI semantics; tested GTK fixtures plus specific Writer and Nautilus workflows, with app-specific gaps |
| [Browser workspace](../apps/browser/README.md) | A managed host Chromium with tabs, structured pages, standard forms, password staging, host files, and persistent profiles | Semantic presentation of supported page controls; original page appearance, canvas, and video are not mirrored |
| [Semantic GTK presentation](../demos/semantic/README.md) | Responsive web controls, guarded Unicode drafts, Boolean values, stable-item selection, dialogs, and redaction for two cooperating apps | Requires a cooperating source and advertised actions; automated layout/accessibility checks do not establish screen-reader usability |
| [Vector GTK mirrors](../demos/web/README.md) | Native and browser rendering of host-owned geometry and outline glyphs, control leases, guarded edits, and recovery | Pinned GTK corpus and declared styles/renderers; unsupported source paint remains explicit |

The desktop workspace can include the managed browser in the same paired session with `--with-browser`.
All paired viewers share the workspace's selected app, and only one device holds remote control at a time.
Clients may reflow semantic controls locally without changing the authoritative application's layout.

## Important limits

Existing-app accessibility reads and action revalidation do not provide an atomic capture or compare-and-set edit operation.
Unmodified apps retain a race between the final check and native execution.
The [desktop guide](../apps/desktop/README.md#design-and-limits) documents identity checks, privacy handling, observation bounds, and ambiguous outcomes.

The strict no-raster profile carries authorized semantics and vectors, excluding screenshots, raster fallbacks, and video frames.
Cooperating vector coverage does not establish arbitrary applications, themes, compositor decorations, or full-desktop mirroring.
The read-only [AT-SPI adapter baseline](../adapters/atspi/README.md) and interactive desktop workspace are separate integrations.

Desktop mobile flows have automated Chromium/WebKit coverage, specific real Writer/Nautilus checks, and a reported successful physical-phone tryout.
A complete device/OS keyboard, IME, orientation, suspension, and assistive-technology matrix remains open.
The standalone browser and cooperating demos retain their own device and two-machine acceptance limits in their guides.

The [performance report](performance.md), [large-workload report](large-performance.md), and [gates](performance-gates.md) record missed original action-latency and large-scene 60 Hz targets.
The browser library has a tested private runtime tarball and compatibility policy; public registry publication and project licensing remain separate release decisions.

## Direction

The immediate product direction is semantic interaction with an existing Linux session from a phone or another browser.
The next work is:

1. Expand real-app compatibility and fill missing accessible names/text capabilities with explicit source evidence.
2. Add bounded paging or window-scoped reads for larger application trees, documents, and collections.
3. Improve managed-browser presentation beyond the current structured reading view.
4. Validate physical mobile keyboards, IME, orientation, suspension, and assistive technology.
5. Reduce measured host/client latency and rendering costs while preserving bounded ownership and recovery.

Application launching, compositor focus/window management, independent application views, and concurrent writers remain separate capabilities requiring their own design and acceptance.

## Prototype acceptance and history

RFC milestones M0–M5 and all 79 original conformance cases are accepted for the declared constrained GTK corpus and isolated EIS session backend.
That includes measured performance evidence, while the provisional 60 Hz budget was missed.
See [implementation acceptance](../conformance/STATUS.md) and the [case audit](../conformance/CASES.md).

[IMPLEMENTATION.md](IMPLEMENTATION.md) and the [P/C/L/S completion records](implementation-plans.md#implementation-record-2026-10-10) describe the subsequent reusable client, producer, performance, and semantic-presentation work.
Their original baselines and remaining-work notes are historical context, rather than a complete description of the newer desktop/browser workspaces.
The [RFC](RFC.md), [conformance.yaml](../conformance.yaml), and [2026-10-02 handoff](HANDOFF.md) preserve earlier design and implementation snapshots.
