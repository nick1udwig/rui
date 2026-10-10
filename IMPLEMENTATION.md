# Implementation status

The authorized scope is RFC milestones M0–M5 and their 79 conformance cases, including the P1 measurement cases.
Deferred features in RFC section 14 remain outside that scope.
Completion requires executable evidence, not the existence of an API or a test with a matching name.
The original design/specification files remain historical inputs; acceptance is tracked in [conformance/STATUS.md](conformance/STATUS.md) and [the case audit](conformance/CASES.md).
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
See [client-browser/README.md](client-browser/README.md) and [demos/web/README.md](demos/web/README.md) for API boundaries, rendering contracts and launch commands.

## Work beyond the accepted prototype

L2's first slice extracts `rui-wire` for generated session schema, bounded codec,
portable errors and redacted/zeroizing ownership. Compatibility transport exports
preserve existing callers and the dependency cycle is removed. The
[behavior matrix](docs/client-behavior-matrix.md) records shared behavior, gaps and
send/cancellation ownership. Full native actor migration remains outstanding;
its existing queue and supported command families are preserved.

C5 adds bounded read-only observed HTML views, explicit stale/unavailable/partial
and redacted state, immediate scope clearing and one disposable worker per source.
Fresh private-bus checks keep a healthy view responsive while another source is
stopped and exclude actual password content. Actions remain disabled; consistency
remains OBSERVED and native visual/input association unresolved.
[The observed integration contract](adapters/atspi/README.md) distinguishes this
local presentation from an authenticated network host or coherent mirror.

C4 records a bounded pinned Hyprland 0.56.2 [feasibility investigation](docs/additional-session-feasibility.md).
Its reviewed capture hooks expose image buffers; trusted registered-content
association and app-only input equivalence remain unproven. The plan's negative
feasibility branch retains the isolated backend and adds no desktop profile.

C3 extracts registered application/surface APIs, common coherent assembly and
control helpers, moving corpus-specific fault hooks out of native dispatch.
The second owned document editor uses the SDK without fixture imports. Generic
source-admitted button, Unicode text/list, dialog/modality and widget replacement
checks pass; private text is redacted and screenshot diagnostics are optional.
Original whole-view, restart, native-context and production HTTPS regressions
pass. [The integration contract](adapters/gtk4/README.md) retains bounded verified
artifact IPC and source/native guards; in-memory IPC and arbitrary GTK applications
remain unclaimed.

C2 adds bounded authored-path lowering for pixel-aligned, opaque square borders
with equal widths and differing side paint. Four fresh source comparisons and
an omitted-side negative pass; Rust replay verifies every retained closure and
returns ownership to zero. Unsupported scale/width/rounded/alpha variants decline.
[The coverage contract](docs/gtk-square-borders.md) preserves the unchanged native
comparison criteria and documents the narrower source-proven subset.

C1 adds the [application/capability matrix](docs/application-matrix.md), a bounded
submitted-node inventory and mandatory unsupported-content companion checks.
A fresh owned-display default/constrained census and six native negative cases
pass; repeated occurrences are separated from unique gap reasons. The selected
second target is an application-owned document editor, pending reusable-adapter
and source/client evidence in C3. Default GTK styling remains explicitly partial.

P5 adds a frozen small-demo evaluator and an opt-in named-runner workflow.
Retained three-run results pass the 16.67 ms draw budget and fail the unchanged
100/150 ms action goals. Ordinary CI validates evidence and correlation rather
than enforcing shared-machine timings. [The gate contract](docs/performance-gates.md)
records the remaining capture cost, runner provisioning, expanded P1 measurements
and large-native 60 Hz gap.

The first P1 series adds bounded opt-in source/host/replica/render timings and a
release browser performance runner with three runs and 200 warm actions per
origin. Timing-ring, causal-result ordering, authority/resource-wait, browser
pixel, producer and actual HTTPS production-bundle checks cover the new paths.
Sustained measurement exposed a lease-renewal/readiness race: renewal now checks
the live lease independently of command readiness, while acquisition and input
remain readiness-gated. The browser also clears its displayed control state on
protocol authority errors. See [the measurement procedure](docs/performance.md).
P1's expanded fault/lifecycle measurement scenarios remain outstanding; this
series does not mark all four workstreams complete.

L1 adds a clean-checkout WASM/browser CI job pinned to Rust 1.98.1, matching
wasm-bindgen, Node 26.8.1 and locked npm dependencies. Node, Chromium 153, Firefox
155 and Playwright WebKit 26.6 execute the generated module; all browser engines
pass the vector/resource/lifecycle suites and served production asset checks.
Fresh owned-display HTTPS checks pass for development and production, including
real hidden-tab release. The separate live job requires provisioning the named
GTK 4.22.4/Cairo 1.18.4 runner. [Retained local evidence](fixtures/browser/ci-local.json)
and [runner requirements](docs/browser-ci.md) describe the tested scope; Safari
and physical mobile devices remain unclaimed.


P2 replaces periodic demo capture with bounded incarnation/generation datagram
wakeups on the serialized source owner. Capture consumption reconciles newer
changes, overflow retains level-triggered dirtiness, and source supervision stays
independent. Burst/overflow/stale-source tests and fresh native restart, context,
whole-view, demo and HTTPS browser checks pass. The retained three-run
[comparison](fixtures/performance/browser-p2/summary.json) records source harness
p95 of 153–154 ms versus 296–302 ms before P2. [Invalidation scope](docs/dirty-notifications.md)
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
[The incremental contract](docs/incremental-gtk.md) and
[exact trace evidence](fixtures/gtk/incremental-publications.json) describe limits;
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
[Reports and limits](docs/browser-render-performance.md) distinguish drawing
from presentation; generation-bound export, damage and the large native cost
remain later measured optimizations.

The [implementation plans](docs/implementation-plans.md) expand these four areas into sequenced changes, dependencies, acceptance criteria and validation commands for future implementors.

- **Performance:** the interactive host now captures from dirty notifications and publishes validated warm transactions. Client state export and rendering still need measured optimization; the larger reference workload also needs renderer optimization and fresh end-to-end measurements. The existing core already supports transactions and resource reuse.
- **Broader source coverage:** the accepted GTK producer cooperates with a pinned application and constrained styles. Default styles still have named unsupported paint nodes; unmodified applications, shell decorations and general compositor desktops require additional adapters and measured coverage. The strict no-raster profile cannot represent arbitrary photos/video faithfully.
- **Browser and library completion:** extend negotiated browser capabilities as needed, consolidate the remaining distinct native/browser connection actors, and reduce state-copy/render overhead. The npm package is private and experimental; release/versioning, broader device compatibility remain future work. WASM/browser CI and the tested desktop engine matrix are now documented separately.
- **Semantic presentation:** the independent semantic-only client proves the schema/action boundary. A semantic UI with a different layout, mobile interaction or accessibility presentation has not been built.

RFC section 14 deliberately defers mobile translation, independent application views, concurrent writers/CRDTs, general GPU streaming, audio/video, clipboard/file transfer, raster asset profiles, executable components, browser-engine capture and cross-compositor standardization. These are optional future scope, not unfinished M0–M5 gates.
