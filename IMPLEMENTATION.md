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


The [implementation plans](docs/implementation-plans.md) expand these four areas into sequenced changes, dependencies, acceptance criteria and validation commands for future implementors.

- **Performance:** the interactive host currently polls every 250 ms and publishes full snapshots. Event-driven source publication and incremental updates can reduce latency and work; the larger reference workload also needs renderer optimization and fresh end-to-end measurements. The existing core already supports transactions and resource reuse.
- **Broader source coverage:** the accepted GTK producer cooperates with a pinned application and constrained styles. Default styles still have named unsupported paint nodes; unmodified applications, shell decorations and general compositor desktops require additional adapters and measured coverage. The strict no-raster profile cannot represent arbitrary photos/video faithfully.
- **Browser and library completion:** extend negotiated browser capabilities as needed, consolidate the remaining distinct native/browser connection actors, and reduce state-copy/render overhead. The npm package is private and experimental; release/versioning, broader device compatibility remain future work. WASM/browser CI and the tested desktop engine matrix are now documented separately.
- **Semantic presentation:** the independent semantic-only client proves the schema/action boundary. A semantic UI with a different layout, mobile interaction or accessibility presentation has not been built.

RFC section 14 deliberately defers mobile translation, independent application views, concurrent writers/CRDTs, general GPU streaming, audio/video, clipboard/file transfer, raster asset profiles, executable components, browser-engine capture and cross-compositor standardization. These are optional future scope, not unfinished M0–M5 gates.
