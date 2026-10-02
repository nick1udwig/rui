# Implementation acceptance

The original root `conformance.yaml` remains an unchanged design artifact.
Passing a component test does not by itself complete an integration case.
The authorized implementation scope is RFC M0–M5, including all 79 specified
cases and the P1 measurements; RFC section 14 remains deferred.

## Accepted foundations

| Area | Executable evidence | Scope |
|---|---|---|
| Schema and reducer | `protocol-core/tests/model.rs`, `scene.rs`; Python oracle | Nine retained primitives; atomic graph/version validation |
| Publications and resources | `publication.rs`, `resources.rs`, `receiver.rs`, `pipeline.rs` | Exact-byte chunks, shared budgets, pins, delayed resources, cache loss and revocation |
| Command and session authority | `commands.rs`, `session.rs`; `host-broker/tests/controlled.rs` | One exact-byte ledger for semantic/raw/composition/resize families; source evidence, leases, private observer projections; full host-family integration pending |
| Input and composition primitives | `protocol-core/tests/input.rs` | Whole-batch validation, ordered capture, conservative cleanup, transient scalar-indexed composition; host integration pending |
| Scoped transcripts | `protocol-core/tests/transcript.rs` | Metadata by default; authorized exact bytes, shared accounting and irreversible revocation |
| Generated hardening | `protocol-core/tests/hardening.rs` | 2,048 replay prefixes and 10,000 bounded malformed-payload mutations |
| Replay minimization | `conformance/shrink.py`, `test_shrink.py` | Independent replay failures produce a bounded, minimized exact-byte transcript preserving the mismatch class |
| Receiver recovery matrix | `protocol-core/tests/recovery_matrix.rs` | Every publication/resource chunk cut, duplicate chunks, cache loss, fresh-state equivalence and 256-resource churn; synthetic command and presentation callbacks, not network/visual evidence |
| Independent decoder/reducer | `python3 conformance/check.py` | 1,275 semantic replay prefixes and 144 scene fixtures across exact bytes and 16 seeds |
| Real TLS/WebSocket binding | `transport/tests/loopback.rs` and transport unit tests | 18 tests: pinned identity, explicit approval, two clients, command retry, private projections, revocation, bounded framing and slow-observer isolation |
| CPU reference renderer | `client-desktop/tests/` | 26 tests: nine primitives, analytic pixels, linear composition, transforms, clipping, surfaces, paths, glyphs, bounds and explicit failure |
| Actual GTK capture and SDK | `adapters/sdk/tests/gtk_capture.rs`, Python producer tests, `fixtures/gtk/measure.py` | Ten strict snapshots, native source glyph evidence, exact resource replay, retirement and bounded artifact decoding |
| GTK local visual comparison | `fixtures/gtk/pixel-comparison.json`, comparator tests and SDK glyph negative controls | Ten strict frames pass declared native/replay tolerance; zero interior mismatches, 942 outline bounds checked; actual decoded glyph omission/shift/size rejected |

Root review required corrections to authorization revocation races, cleanup
ordering and panic handling, recursive allocation preflight, private transport
logging, conservative input cleanup, resource descriptor access and renderer
work bounds. The accepted CPU renderer also passes against a fresh checksum-pinned
Cairo 1.18.4 build with optional native backends disabled, as used by CI.

## Milestone gates

- **M0 accepted for the declared corpus.** [Measured GTK coverage](../docs/gtk-coverage.md)
  identifies the actual same-node snapshot boundary, cooperating stable semantics,
  ten frames per style, 61 default-style gaps and zero constrained-style gaps.
  Window decorations and compositor output are outside that capture scope.
- **M1 remains open.** Complete all controlled command families, post-auth transcript
  integration, independent result interoperability and the full core-case audit.
- **M2 remains open.** The graphical host fixture, two actual desktop mirrors,
  controls/text/list/scroll/transients/canvas, authorized resize, resource-bearing
  transport, retained reconnect and host/reference comparisons must work together.
- **M3 remains open.** Typed source interaction, focus/IME, explicit popup lifecycle
  and guarded remote source dispatch remain. The declared ten-frame local visual
  comparison has passed; whole-view composition with coherent transients remains.
- **M4 remains open.** Implement and test the explicitly supported isolated session
  shell, permissioned libei input, geometry/modality/focus/scaling, takeover,
  observed accessibility reconciliation and enforceable scope.
- **M5 remains open.** Complete the independent semantic client, network fault and
  recovery traces, sustained resource churn, case-by-case acceptance audit and
  integrated named-hardware CPU/latency/bandwidth/memory measurements.

Current transport foundation limits and trusted boundaries are in its
[README](../transport/README.md). The CPU renderer's native allocation policy,
local image artifacts and pinned sampling are specified in
[its profile](../spec/renderer-profile.md). Native GTK antialiasing differences
are covered by an explicit calibrated tolerance and independent glyph checks;
this does not extend acceptance beyond the measured constrained corpus.
