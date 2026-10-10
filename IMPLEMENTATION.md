# Implementation objectives

The authorized scope is RFC milestones M0–M5 and their 79 conformance cases, including the P1 measurement cases.
Deferred features in RFC section 14 remain outside that scope.
Completion requires executable evidence, not the existence of an API or a test with a matching name.
The original design/specification files remain historical inputs; acceptance is tracked separately.

| Gate | Required result | Current remaining work |
|---|---|---|
| M0 | Pinned GTK corpus, actual shared capture boundary, measured coverage and no-raster report | Accepted for the declared corpus; see docs/gtk-coverage.md |
| M1 | GUI/network-independent core cases, schema, exact transcripts, independent replay and generated properties | Accepted for the declared model corpus; independent reducer/property/network replay and frozen core regression pass |
| M2 | Local authoritative rendering and two desktop mirrors, controls/text/list/scroll/transients/canvas, reconnect/cache recovery, local-only resize | Accepted for the controlled corpus; actual source/two-mirror controls, transient surfaces, typing, independent Python actions, cache loss/reconnect and local-only viewport changes pass |
| M3 | Real GTK producer with explicit identities, source evidence, lifecycle/action/no-raster checks | Actual GTK source bindings, coherent capture, native actions, lifecycle/restart, two TLS mirrors and no-raster audits accepted; native wire composition and desktop IME integration remain |
| M4 | Explicitly supported isolated compositor/session with geometry/modality/focus/scaling, local takeover and permissioned region input | Actual isolated EIS source/two-mirror baseline, host layout/repeat, topology, cursor and unequal input geometry accepted; direct/local click comparison, stale-wire routing, app-only network denial remain; four causal held-input cleanup variants accepted |
| M5 | Independent semantic-only client, fault injection, fuzz/property/churn tests, named-hardware measurements | Independent semantic client, generated properties, actual TLS fault/churn checks and named-hardware CPU/network/native latency measurements accepted; final case audit and combined regression remain |

Current parallel tracks extend the accepted foundations: desktop mirrors, typed GTK source interactions and lifecycle, and the common command ledger plus expanded host/transport integration.
Input/IME interpretation and acceptance integration are managed alongside those tracks.
Features are reviewed and corrected before logical commits.
Milestones remain open until their complete gate is demonstrated.
See [the complete case audit](conformance/CASES.md) for evidence and remaining work for every specified case.
