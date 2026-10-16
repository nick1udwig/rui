# Implementation log

This log records the original prototype and subsequent P/C/L/S implementation work.
Entries retain the remaining-work notes from their implementation stage.
For the current desktop/browser workspaces and priorities, see [status and next steps](status.md).
Use [development and verification](development.md) for checks and the [documentation index](README.md) for component guides.

P1 ownership diagnostics: matching crate/browser 0.1.1 adds a conservative Rust
memoryPeak getter for cold/recovery/lifecycle reports, separate from JavaScript and
socket/allocator capacity. Core and three-engine disposed/active ownership checks
pass; the generated binding/API baseline is updated without changing wire limits.

L5 local distribution implemented: runtime-only private ESM/declaration/WASM tarball,
version/schema/ABI/asset compatibility gate, pre-1.0 changelog and locked dependency
notices. A clean Vite consumer installs outside the repo and serves under /apps/rui/.
All three engines pass initialization/foreign-ABI/size/MIME/CSP and disposal checks.
Fresh actual GTK checks exercise default/verifying-proxy and replacement/direct-WSS
adapters, guarded Unicode edits, source edits, release and explicit reconnect.
Recreated actors receive fresh random scoped control request IDs; discarded producer
sequences remain rejected. See docs/browser-distribution.md and fixtures/browser/distribution.
No registry publication/project license is selected; two-machine tailnet and physical
mobile/screen-reader acceptance remain unverified and require actual test devices.

L2 full actor migration: native NetworkClient and portable Session now delegate
negotiation, publication/resource transitions, readiness, leases, resume and exact
command ownership to client-core Replica. Native cancellation-safe socket admission
remains in the transport adapter. Dual-prefix/core tests, native TLS lost-result
recovery and all existing native gates pass. GTK incremental publications withdraw
the optional native clear/insert clock profile and use canonical public clocks;
fresh native IME proves four edits, ten exact painted frames and zero disposal
ownership. See docs/client-behavior-matrix.md and fixtures/gtk/shared-actor.
Distributed-package/external-machine acceptance remains L5.

L2/L3 resource slice: native and portable clients share the bounded request cursor;
portable live cache loss/eviction and retained cache-loss resume preserve records and
exact commands while pausing readiness until verified closure/presentation. Public
browser APIs expose owned resource copies, resource readiness and explicit full-frame
refresh. A 512-body/1024-byte-message regression, native TLS recovery/cancellation,
three-engine WASM tests and fresh owned editor recovery pass. The subsequent full actor migration is recorded above; distributed external-consumer
acceptance remains outstanding.

S5 implemented for two owned sources: distinct approved semantic/mirror clients
perform the generic three-effect scenario, exchange one lease and converge. Fresh
exact traces independently replay both authorized projections through reset/reconnect,
delayed delivery, stale capture, source geometry changes, source replacement and
read-only permission reduction. The editor additionally proves native modal changes,
retirement/rematerialization and two-of-ten partial collection semantics. This found
and fixed missing registered size invalidation. Trusted source replacement switches
the native fence and revokes control while retaining the broker/ledger. See
fixtures/gtk/presentation-boundary and demos/semantic/README.md. Strict color glyphs
remain declined; real device/external-machine claims remain unverified.

S4 local implementation: responsive semantic groups/forms/collections and named
source dialogs, automatic text direction, associated action names/descriptions,
distinct local/source focus and live pending/error announcements. Two applications
pass 320/800/1440 px, 200% text, keyboard and browser accessibility checks in all
three engines, with unchanged source data and no reflow/navigation commands.
Screenshots were visually inspected. See fixtures/browser/semantic-layout; physical
screen-reader/mobile-device acceptance remains unverified and is not claimed.

S3 implemented: optional generic SemanticControls maps advertised schema-1 actions
to guarded public APIs. Explicit source focus precedes immutable edit capture; local
composition/paste/deletion remain draft state until Apply. Authoritative values stay
visible while pending; stale contexts do not silently rebase. Revocation/removal/
scope cleanup purge drafts and ambiguous native outcomes block fresh commands.
Three-engine DOM tests and fresh owned corpus/editor checks pass, including independent
Python exact-wire replay. See demos/semantic/README.md and
fixtures/gtk/semantic-controls-golden. Simultaneous mirror convergence remains S5.

Command ambiguity correction: portable exact command bytes remain retained for
Unknown/OutcomeUnknown/OutcomeExpired and Dispatched, and causal waiting recognizes
the native status names. A renderer/readiness update alone cannot unlock a fresh edit;
explicit recovery resends original bytes. Rust queue and three-engine browser tests
cover the native ambiguity vocabulary.

S2 implemented: bounded read-only semantic DOM mapping and separate demos/semantic
consumer, with role/ownership/group/reading-order layout, explicit unavailable/redacted
text and partial collections, safe remote strings and stable protocol identity keys.
Three-engine tests cover renamed labels, new IDs/arbitrary geometry, local focus,
unmapped/removal cleanup and limits. Live corpus/editor checks render before readiness
and compare exact wire state with Python. See demos/semantic/README.md. Interactive
DOM controls and responsive/accessibility acceptance remain S3/S4 gates.

L4 enabled semantic increment: typed Boolean SetValue and explicit stable-item/model
activation APIs, scalar text/selection parameter bounds, a registered native boolean
source control and private dispatcher/host Boolean schema. Undeclared schemas/types
and stale collection versions fail before dispatch; release blocks value mutation.
Raw/browser keyboard/composition/resize/fingerprint capabilities remain disabled,
with the tested subset and limitations recorded in docs/browser-capabilities.md.

S1 implemented: explicit portable/browser semantic-only negotiation and SemanticReady;
no canvas/glyph requirement, Presented/hit-test rejection, exact readiness/authority
clearing and owned capability metadata. Actual WASM transcripts pass in three engines;
the live corpus and editor complete semantic button/Unicode/stable-item effects and
reset/reconnect without authority restoration. Independent Python exact-wire replay
matches each final projection (six prefixes each across two scopes). See
docs/semantic-session.md and fixtures/gtk/semantic-session-golden.

L3 contract slice: complete Rust model declarations and owned state copies are
available to custom transports/renderers. Concurrent credential requests reject;
listener disposal stops old-generation processing. The complete Python/native/WASM
serialization fixture covers all six records and maximum-width clocks/typed values.
See docs/browser-integration.md. Semantic configuration, capability reporting,
cache-loss APIs and clean external consumer validation are still tracked separately.

## Accepted prototype

The historical prototype acceptance covers RFC milestones M0–M5 and their 79 conformance cases, including the original performance cases.
Deferred features in RFC section 14 remain outside that scope.
Completion requires executable evidence, not the existence of an API or a test with a matching name.
The original design/specification files remain historical inputs; acceptance is tracked in [conformance/STATUS.md](../conformance/STATUS.md) and [the case audit](../conformance/CASES.md).
All six milestones and all 79 cases are accepted for the declared constrained GTK corpus and isolated session backend. Performance was measured; the provisional 60 Hz target was not met.

| Gate | Required result | Accepted result |
|---|---|---|
| M0 | Pinned GTK corpus, actual shared capture boundary, measured coverage and no-raster report | Accepted for the declared corpus; see docs/gtk-coverage.md |
| M1 | GUI/network-independent core cases, schema, exact transcripts, independent replay and generated properties | Accepted for the declared model corpus; independent reducer/property/network replay and frozen core regression pass |
| M2 | Local authoritative rendering and two desktop mirrors, controls/text/list/scroll/transients/canvas, reconnect/cache recovery, local-only resize | Accepted for the controlled corpus; actual source/two-mirror controls, transient surfaces, typing, independent Python actions, cache loss/reconnect and local-only viewport changes pass |
| M3 | Real GTK producer with explicit identities, source evidence, lifecycle/action/no-raster checks | Same-tree capture, guarded native actions, coherent whole-view lifecycle/restart and actual desktop wire composition/IME accepted; independent exact network audits and source/two-mirror comparisons retained |
| M4 | Explicitly supported isolated compositor/session with geometry/modality/focus/scaling, local takeover and permissioned region input | Actual isolated EIS topology/input, host layout/repeat, direct/local equivalence, stale-wire routing, app-only denial and four held-input cleanup causes accepted; AT-SPI remains explicitly OBSERVED |
| M5 | Independent semantic-only client, fault injection, fuzz/property/churn tests, named-hardware measurements | Independent semantic client, generated properties, actual TLS fault/churn checks, final case audit and combined regression accepted; named-hardware measurements document performance limits |

## Browser and visual playground extension

The manual GTK host/client demo makes the accepted native implementation interactive, including host-side edits, network binding, retained reconnect and fresh temporary credentials.
The Vite demo adds a browser client that can run on another machine. It consumes an experimental reusable TypeScript library backed by the actual Rust receiver and portable session actor through WASM.
The browser supports all nine vector primitives, source outline glyphs, control leases, activation/focus, guarded text replacement, verified resources and retained reconnect. Its live owned-display test covers HTTPS/WSS, host/client edits, observer restrictions and hidden-tab release/resume.
It does not inherit every native client capability: raw input, source IME/preedit, fingerprint-font shaping, cursors and session-shell controls remain unadvertised. DOM drafts can submit committed Unicode text.
See [client-browser/README.md](../client-browser/README.md) and [demos/web/README.md](../demos/web/README.md) for API boundaries, rendering contracts and launch commands.

## Work beyond the accepted prototype

L2's first slice extracts `rui-wire` for generated session schema, bounded codec,
portable errors and redacted/zeroizing ownership. Compatibility transport exports
preserve existing callers and the dependency cycle is removed. The
[behavior matrix](client-behavior-matrix.md) records shared behavior, gaps and
send/cancellation ownership. The subsequent shared Replica migration removes duplicate decoding/transition logic;
its native socket queue and supported command families remain adapter contracts.

C5 adds bounded read-only observed HTML views, explicit stale/unavailable/partial
and redacted state, immediate scope clearing and one disposable worker per source.
Fresh private-bus checks keep a healthy view responsive while another source is
stopped and exclude actual password content. Actions remain disabled; consistency
remains OBSERVED and native visual/input association unresolved.
[The observed integration contract](../adapters/atspi/README.md) distinguishes this
local presentation from an authenticated network host or coherent mirror.

C4 records a bounded pinned Hyprland 0.56.2 [feasibility investigation](additional-session-feasibility.md).
Its reviewed capture hooks expose image buffers; trusted registered-content
association and app-only input equivalence remain unproven. The plan's negative
feasibility branch retains the isolated backend and adds no desktop profile.

C3 extracts registered application/surface APIs, common coherent assembly and
control helpers, moving corpus-specific fault hooks out of native dispatch.
The second owned document editor uses the SDK without fixture imports. Generic
source-admitted button, Unicode text/list, dialog/modality and widget replacement
checks pass; private text is redacted and screenshot diagnostics are optional.
Original whole-view, restart, native-context and production HTTPS regressions
pass. [The integration contract](../adapters/gtk4/README.md) retains bounded verified
artifact IPC and source/native guards; in-memory IPC and arbitrary GTK applications
remain unclaimed.

C2 adds bounded authored-path lowering for pixel-aligned, opaque square borders
with equal widths and differing side paint. Four fresh source comparisons and
an omitted-side negative pass; Rust replay verifies every retained closure and
returns ownership to zero. Unsupported scale/width/rounded/alpha variants decline.
[The coverage contract](gtk-square-borders.md) preserves the unchanged native
comparison criteria and documents the narrower source-proven subset.

C1 adds the [application/capability matrix](application-matrix.md), a bounded
submitted-node inventory and mandatory unsupported-content companion checks.
A fresh owned-display default/constrained census and six native negative cases
pass; repeated occurrences are separated from unique gap reasons. The selected
second target is an application-owned document editor, pending reusable-adapter
and source/client evidence in C3. Default GTK styling remains explicitly partial.

P5 adds a frozen small-demo evaluator and an opt-in named-runner workflow.
Retained three-run results pass the 16.67 ms draw budget and fail the unchanged
100/150 ms action goals. Ordinary CI validates evidence and correlation rather
than enforcing shared-machine timings. [The gate contract](performance-gates.md)
records the remaining capture cost, runner provisioning, expanded P1 measurements
and large-native 60 Hz gap.

The first P1 series adds bounded opt-in source/host/replica/render timings and a
release browser performance runner with three runs and 200 warm actions per
origin. Timing-ring, causal-result ordering, authority/resource-wait, browser
pixel, producer and actual HTTPS production-bundle checks cover the new paths.
Sustained measurement exposed a lease-renewal/readiness race: renewal now checks
the live lease independently of command readiness, while acquisition and input
remain readiness-gated. The browser also clears its displayed control state on
protocol authority errors. See [the measurement procedure](performance.md).
P1's expanded fault/lifecycle measurement scenarios remain outstanding; this
series does not mark all four workstreams complete.

L1 adds a clean-checkout WASM/browser CI job pinned to Rust 1.98.1, matching
wasm-bindgen, Node 26.8.1 and locked npm dependencies. Node, Chromium 153, Firefox
155 and Playwright WebKit 26.6 execute the generated module; all browser engines
pass the vector/resource/lifecycle suites and served production asset checks.
Fresh owned-display HTTPS checks pass for development and production, including
real hidden-tab release. The separate live job requires provisioning the named
GTK 4.22.4/Cairo 1.18.4 runner. [Retained local evidence](../fixtures/browser/ci-local.json)
and [runner requirements](browser-ci.md) describe the tested scope; Safari
and physical mobile devices remain unclaimed.


P2 replaces periodic demo capture with bounded incarnation/generation datagram
wakeups on the serialized source owner. Capture consumption reconciles newer
changes, overflow retains level-triggered dirtiness, and source supervision stays
independent. Burst/overflow/stale-source tests and fresh native restart, context,
whole-view, demo and HTTPS browser checks pass. The retained three-run
[comparison](../fixtures/performance/browser-p2/summary.json) records source harness
p95 of 153–154 ms versus 296–302 ms before P2. [Invalidation scope](dirty-notifications.md)
excludes unregistered animation/custom-paint changes rather than promising their
detection.

P3 publishes warm GTK transactions from bounded per-view canonical projections.
The reducer validates every candidate against an equivalent snapshot, preserving
retirements and the exact authorized resource closure. Public clocks remain
separate from captured native clocks; old public revisions cannot be translated
into a fresh native guard after an ABA. Snapshot fallback covers initial views,
source replacement and recovery. Fresh native Unicode, restart, whole-view,
selection/focus, TLS composition and headed production HTTPS browser checks pass.
The browser's draft capture now waits for the current presentation opportunity
and for an admitted command's causal frame when its result arrives first.
Larger-than-message publication and every transaction chunk cut, missing-resource,
cache-loss, stalled-view and allocation regressions pass. Independent Python/Rust
replay agrees at all eight retained prefixes (one snapshot, seven transactions).
[The incremental contract](incremental-gtk.md) and
[exact trace evidence](../fixtures/gtk/incremental-publications.json) describe limits;
GTK retained reconnect and general permission-redaction projections remain outside
this adapter's scope.

P4's first measured optimization reuses browser linear/output buffers, keeps
coverage across paint-only changes, and removes per-pixel gradient allocations.
It preserves full-frame linear composition and the native renderer contract.
Owned graph/key/buffer overlap is charged under the unchanged ceiling, and scope
reset/disconnect clears JS state and derived pixels/caches. All three engines
pass exact cached/reference comparisons and lifecycle/ownership regressions;
production HTTPS host/browser edits, hidden-tab release and reconnect pass.
Three 600-frame synthetic runs reduce draw p95 from 5.0–5.3 ms to 3.4–3.5 ms.
The three 200-action demo comparisons show reduced composition cost, but the
source-action harness remains about 153 ms p95 and browser latency varies.
[Reports and limits](browser-render-performance.md) distinguish drawing
from presentation; generation-bound export, damage and the large native cost
remain later measured optimizations.

The [implementation plans](implementation-plans.md) preserve the sequenced changes, dependencies, acceptance criteria, validation commands, and completion evidence for these four workstreams.
Their starting-point descriptions are historical baselines.
Current priorities and coverage limits, including the newer existing-app and managed-browser workspaces, are collected in [status and next steps](status.md).

RFC section 14 records features deferred from the original M0–M5 scope.
Subsequent semantic presentation and managed-browser work extend that baseline; the original deferral list is not a current inventory of unavailable features.
