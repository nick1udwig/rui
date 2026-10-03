# Implementation acceptance

The original root `conformance.yaml` remains an unchanged design artifact.
Passing a component test does not by itself complete an integration case.
The authorized implementation scope is RFC M0–M5, including all 79 specified
cases and the P1 measurements; RFC section 14 remains deferred.
The [case-by-case audit](CASES.md) maps all 79 cases to committed evidence and
remaining integration gates.

## Accepted foundations

| Area | Executable evidence | Scope |
|---|---|---|
| Schema and reducer | `protocol-core/tests/model.rs`, `scene.rs`; Python oracle | Nine retained primitives; atomic graph/version validation |
| Publications and resources | `publication.rs`, `resources.rs`, `receiver.rs`, `pipeline.rs` | Exact-byte chunks, shared budgets, pins, delayed resources, cache loss and revocation |
| Command and session authority | `commands.rs`, `session.rs`; `host-broker/tests/controlled.rs` | One exact-byte ledger for semantic/raw/composition/resize families; source evidence, leases, private observer projections; headless/TLS source dispatch for all four families accepted |
| Input and composition primitives | `protocol-core/tests/input.rs` | Whole-batch validation, ordered capture, conservative cleanup, transient scalar-indexed composition; headless/TLS integration includes actual source canvas/resize, transient preedit and scalar commits; native adapters remain separately gated |
| Controlled source input families | `host-broker/tests/input_families.rs`, `transport/tests/input_families.rs` | Eight headless and three real TLS cases: one exact-byte sequence ledger, permission-filtered modes, source canvas effects, scalar composition, source resize, private preedit, stale routes/leases, cleanup and cache-loss neutralization; graphical/native dispatch remains separate |
| Scoped transcripts | `protocol-core/tests/transcript.rs`, `transport/tests/diagnostics.rs` | Opt-in post-auth TLS recording: metadata mode or explicitly authorized exact pre-encryption bytes; same-view reserved budget, bounded connections/records, credential exclusion, scoped inspection and revocation/close cleanup; actual private-entry observer publication/resource/history scans pass in both recording modes; full native trace audit remains |
| Generated hardening | `protocol-core/tests/hardening.rs` | 2,048 replay prefixes and 10,000 bounded malformed-payload mutations |
| Replay minimization | `conformance/shrink.py`, `test_shrink.py` | Independent replay failures produce a bounded, minimized exact-byte transcript preserving the mismatch class |
| Actual TLS recovery matrix | `transport/tests/fault_matrix.rs`, `common/fault_proxy.rs` | Every actual cold snapshot/resource and warm command/delta packet boundary: 34+4 cuts, lost command-result exact retry, old-lease rejection, unchanged prior records, full-canvas CPU source/recovered pixel equality and delayed delivery; native GUI recovery remains |
| Sustained TLS ownership churn | `transport/tests/sustained.rs` | 128 surface incarnations, 384 effects, 32 reconnects, 430 evictions and 512 retired IDs; source equality/live cache/staging/queue checks each cycle, lease renewal, four-client fail-closed retention and exact retry after churn; receiver budgets return to zero |
| Integrated load measurements | `transport/examples/measure_integrated.rs`, `client-desktop/tests/performance_source.rs`; [report](../docs/performance.md) | 10,000 records and 2,500 visible native glyph cells; exact one/twenty-record deltas, two TLS receivers, stalled-observer isolation, p50/p95/p99 encode/apply/render, warm cache reuse and bounded lifetime allocation peaks; CPU rendering misses 60 Hz, native presentation remains separate |
| Receiver recovery matrix | `protocol-core/tests/recovery_matrix.rs` | Every publication/resource chunk cut, duplicate chunks, cache loss, fresh-state equivalence and 256-resource churn; synthetic command and presentation callbacks, not network/visual evidence |
| Independent decoder/reducer | `python3 conformance/check.py` | 1,275 semantic replay prefixes and 144 scene fixtures across exact bytes and 16 seeds |
| Real TLS/WebSocket binding | `transport/tests/loopback.rs` and transport unit tests | Pinned identity, explicit approval, two clients, command retry, private projections, revocation, bounded framing and slow-observer isolation; cancelled receives and polled flushes retain budgeted replies without duplicate frames |
| Authorized network resources | `transport/tests/resources.rs`, `protocol-core/tests/session.rs` | Two TLS clients, paged cold closure, warm reuse, live cache loss, premature acknowledgments, invalid eviction and revocation; exact retained transaction acknowledgment survives history eviction; ordinary retired eviction now passes (`transport/tests/eviction.rs`), including reservation failure, bounded churn and retained reconnect |
| Retained authenticated reconnect | `transport/tests/reconnect.rs`, `host-broker/tests/controlled.rs`, `protocol-core/tests/receiver.rs` | Eight real TLS cases cover warm/offline replay, higher checkpoint preserving retirement/clocks, interrupted chunks/exact retry, cache loss, fresh scope on changed limits/reapproval, immediate reconnect/cancellation and allocation failure; immutable approval lifetime denies offline reads and prunes history; GUI recovery remains |
| Independent semantic client | `conformance/semantic_client.py`, `test_semantic_client.py`, `typed_interop.py`; `transport/tests/independent.rs` | Standard-library Python decoder, reducer and TLS client; six typed replay prefixes agree with Rust; real source activation, Unicode replacement, stable-item selection and exact retry without scenes or Presented; combined graphical-observer integration remains |
| CPU reference renderer | `client-desktop/tests/pixels.rs`, `cli.rs` | Nine primitives, analytic pixels, linear composition, transforms, clipping, surfaces, paths, glyphs, bounds and explicit failure |
| Guarded GTK mirror | `client-desktop/src/desktop.rs`, `tests/interaction.rs`; `fixtures/render/native_smoke.py` | Actual GTK frame callbacks and two-mirror TLS smoke; generic advertised actions, scalar text context guards, bounded frame ownership and local observer transforms; bounded causal FIFO, visible native preedit and sticky input failure feedback; full-canvas two-window source/record/pixel convergence and independent Python item effects pass; broader interaction/recovery remain |
| Expanded controlled source | `host-broker/tests/corpus.rs`; `client-desktop/tests/corpus_reference.rs` | Unicode edits, checkbox, stable collections, clipped scrolling and fresh transient identities; two TLS replicas equal direct source records and selected vector renders; absent font provider stays explicitly Partial and no Presented is claimed |
| Pinned source font helper | `fixtures/render/native_font.py`, `test_native_font.py` | Private pinned font configuration, bounded actual Pango positions and RVP1 outlines; bounded host closures retain authorized glyph bodies across publication owners; paged real TLS delivery and actual native font/two-mirror gate accepted |
| Isolated native session foundation | `session-shell/tests/native.rs`, `tests/topology.rs`; `fixtures/session-shell/native-source.json` | Nine actual fd-only EIS tests and five topology/reducer tests: explicit unequal regions, actual XKB layout/repeat, topmost hit/modal guards, source effects and verified releases/cancels/STOP; resource readiness loss neutralizes repeats; no native local-device, TLS or actual mirror Presented claim |
| Pinned vector cursor | `client-desktop/tests/cursor.rs`, `spec/cursor-profile.md` | Explicit optional profile, bundled vector shapes, authorized custom paths, hotspot and canvas clipping; native session integration remains |
| Causal typing queue | `client-desktop/tests/typing.rs` | Bounded FIFO, one in-flight edit, exact result plus source text/version/selection confirmation and actual Presented gate; six rapid native characters, repeated focus, Unicode preedit and sticky full-channel rejection pass |
| Trusted resource admission | `protocol-core/tests/receiver.rs` | Authenticated producer helper validates the complete graph before granting its declared closure; invalid graphs and allocation failures roll back grants; strict default receiver unchanged |
| Actual GTK capture and SDK | `adapters/sdk/tests/gtk_capture.rs`, Python producer tests, `fixtures/gtk/measure.py` | Twenty strict publications: seventeen mapped snapshots and three lifecycle signals; typed scalar text/selection, collection and focus clocks, unmap/remap pin release, fresh identities after destruction, reentrant lifecycle ordering and bounded artifact decoding |
| Guarded native GTK commands | `adapters/sdk/tests/native_trace.rs`, `fixtures/gtk/native_check.py` | Common-ledger admission, live native identity/clocks/modality/lease guards, causal capture, exact retries and unknown-outcome retention; 32 dispatches/37 callbacks, 47 source publications and 273 hash-bound vector artifacts; bounded semantic fixture, no TLS/IME claim |
| Native Unicode scalar editing | `adapters/sdk/tests/unicode_trace.rs`, `fixtures/gtk/unicode-interaction.json` | Nine source captures verify supplementary/combining/RTL text, backward and intra-cluster selection, carets, unchanged selection-only content clocks and exact retry; one native scalar-range call fixes grapheme-based selection loss; no wire IME claim |
| Native source restart | `adapters/sdk/tests/restart_trace.rs`, `fixtures/gtk/restart-interaction.json` | Actual post-callback SIGKILL preserves the broker epoch/common ledger; exact ambiguous retries perform no new effects, old app IDs retire, replacement identities/clocks are fresh, delayed generation dispatch fails, three failed starts preserve unavailable state and final ownership returns to zero; bounded private semantic fixture |
| GTK unsupported sources and vector audit | `adapters/sdk/tests/negative_trace.rs`, `fixtures/gtk/negative-report.json`; refreshed native trace | Actual photograph, texture, Cairo, blur and shader nodes decline with named Partial regions; native path control succeeds; observed Python exporter traps, exact decoded geometry/resources and gap census checks; full network recorder remains open |
| Coherent native whole-view capture | `adapters/sdk/tests/whole_trace.rs`, `fixtures/gtk/whole-interaction.json`, `test_transaction.py` | Five real four-surface/lifecycle transactions, fresh per-surface snapshot barrier, interrupted-union abort, typed focus/modal topology, exact vector closure and five direct native/replay pixel comparisons; bounded quota returns to zero; remote whole-view command path remains |
| GTK local visual comparison | `fixtures/gtk/pixel-comparison.json`, comparator tests and SDK glyph negative controls | Seventeen strict frames pass declared native/replay tolerance; zero interior mismatches, 1,577 outline bounds checked; actual decoded glyph omission/shift/size rejected; reports bind exact golden envelope bytes |

Root review required corrections to authorization revocation races, cleanup
ordering and panic handling, recursive allocation preflight, private transport
logging, conservative input cleanup, resource descriptor access and renderer
work bounds. The accepted CPU renderer also passes against a fresh checksum-pinned
Cairo 1.18.4 build with optional native backends disabled, as used by CI.

## Milestone gates

- **M0 accepted for the declared corpus.** [Measured GTK coverage](../docs/gtk-coverage.md)
  identifies the actual same-node snapshot boundary, cooperating stable semantics,
  seventeen mapped frames per style, 104 default-style gaps and zero constrained-style gaps.
  Window decorations and compositor output are outside that capture scope.
- **M1 remains open.** Controlled host/TLS dispatch for all four families passes. Complete the full
  native recorded interaction audit, independent result interoperability and core-case audit.
- **M2 remains open.** Resource-bearing full-canvas source/two-native-mirror
  comparison, rapid typing/preedit, independent Python item selection and local
  observer transforms have passed. Complete broader controls/transients, custom
  canvas effects, source resize and native GUI reconnect/cache recovery. Retained
  TLS reconnect/cache recovery has passed separately.
- **M3 remains open.** Typed capture, explicit popup lifecycle, guarded native
  semantic dispatch, causal action evidence and observed unsupported-source/vector
  audits and actual broker-preserving native source restart have passed review.
  Wire IME, full native interaction recording and remote whole-view command integration
  remain. Coherent whole-view
  native capture and all five direct/replay pixel comparisons have passed. The declared seventeen-frame local
  visual comparison has passed.
- **M4 remains open.** The isolated fd-only native shell foundation passes actual EIS,
  source geometry/hit testing, host XKB/repeat and verified cleanup gates. Complete
  native local-device capture, shared transport authority, actual mirror presentation,
  observed accessibility reconciliation and the final scope/integration audit.
- **M5 remains open.** Extend accepted Python item/two-mirror convergence to button/text;
  complete native action-to-Presented measurements and the case-by-case acceptance
  audit. Actual TLS fault/recovery traces, sustained resource churn and integrated
  named-hardware CPU/network/bandwidth/memory measurements have passed.

Current transport foundation limits and trusted boundaries are in its
[README](../transport/README.md). The CPU renderer's native allocation policy,
local image artifacts and pinned sampling are specified in
[its profile](../spec/renderer-profile.md). Native GTK antialiasing differences
are covered by an explicit calibrated tolerance and independent glyph checks;
this does not extend acceptance beyond the measured constrained corpus.
