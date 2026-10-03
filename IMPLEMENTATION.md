# Implementation objectives

The authorized scope is RFC milestones M0–M5 and their 79 conformance cases,
including the P1 measurement cases. Deferred features in RFC section 14 remain
outside that scope. Completion requires executable evidence, not the existence
of an API or a test with a matching name. The original design/specification files
remain historical inputs; acceptance is tracked separately.

| Gate | Required result | Current remaining work |
|---|---|---|
| M0 | Pinned GTK corpus, actual shared capture boundary, measured coverage and no-raster report | Accepted for the declared corpus; see docs/gtk-coverage.md |
| M1 | GUI/network-independent core cases, schema, exact transcripts, independent replay and generated properties | All four controlled host/TLS command families and scoped recorder accepted; complete native interaction trace and final core-case audit |
| M2 | Local authoritative rendering and two desktop mirrors, controls/text/list/scroll/transients/canvas, reconnect/cache recovery, local-only resize | Source glyph delivery, rapid native typing/preedit and complete-canvas two-mirror/source comparisons accepted; retained TLS reconnect/cache recovery accepted; broader controls/transients, source resize and native GUI recovery remain |
| M3 | Real GTK producer with explicit identities, source evidence, lifecycle/action/no-raster checks | Typed capture, source clocks, guarded native semantic dispatch, popup lifecycle and seventeen-frame comparison accepted; observed unsupported-source/vector audit accepted; coherent native whole-view capture accepted; wire IME, source restart and network whole-view commands remain |
| M4 | Explicitly supported isolated compositor/session with geometry/modality/focus/scaling, local takeover and permissioned region input | Owned fd-only EIS shell source, geometry/hit testing, host XKB/repeat and verified cleanup accepted; native local-device capture, shared transport authority and actual mirror presentation remain |
| M5 | Independent semantic-only client, fault injection, fuzz/property/churn tests, named-hardware measurements | Independent Python Unicode/stable-item actions, actual TLS fault/churn traces and 10,000-record CPU/network measurements accepted; combined graphical convergence, native action-to-Presented measurements and final case audit remain |

Current parallel tracks extend the accepted foundations: desktop mirrors,
typed GTK source interactions and lifecycle, and the common command ledger plus
expanded host/transport integration. Input/IME interpretation and acceptance
integration are managed alongside those tracks. Features are reviewed and
corrected before logical commits. Milestones remain open until their complete
gate is demonstrated.
See [the complete case audit](conformance/CASES.md) for evidence and remaining
work for every specified case.
