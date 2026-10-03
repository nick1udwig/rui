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
| Command and session authority | `commands.rs`, `session.rs`; `host-broker/tests/controlled.rs` | One exact-byte ledger for semantic/raw/composition/resize families; source evidence, leases, private observer projections; full host-family integration pending |
| Input and composition primitives | `protocol-core/tests/input.rs` | Whole-batch validation, ordered capture, conservative cleanup, transient scalar-indexed composition; host integration pending |
| Scoped transcripts | `protocol-core/tests/transcript.rs` | Metadata by default; authorized exact bytes, shared accounting and irreversible revocation |
| Generated hardening | `protocol-core/tests/hardening.rs` | 2,048 replay prefixes and 10,000 bounded malformed-payload mutations |
| Replay minimization | `conformance/shrink.py`, `test_shrink.py` | Independent replay failures produce a bounded, minimized exact-byte transcript preserving the mismatch class |
| Receiver recovery matrix | `protocol-core/tests/recovery_matrix.rs` | Every publication/resource chunk cut, duplicate chunks, cache loss, fresh-state equivalence and 256-resource churn; synthetic command and presentation callbacks, not network/visual evidence |
| Independent decoder/reducer | `python3 conformance/check.py` | 1,275 semantic replay prefixes and 144 scene fixtures across exact bytes and 16 seeds |
| Real TLS/WebSocket binding | `transport/tests/loopback.rs` and transport unit tests | Pinned identity, explicit approval, two clients, command retry, private projections, revocation, bounded framing and slow-observer isolation; cancelled receives and polled flushes retain budgeted replies without duplicate frames |
| Authorized network resources | `transport/tests/resources.rs`, `protocol-core/tests/session.rs` | Two TLS clients, paged cold closure, warm reuse, live cache loss, premature acknowledgments, invalid eviction and revocation; exact retained transaction acknowledgment survives history eviction; historical unpinned eviction and retained reconnect remain |
| Independent semantic client | `conformance/semantic_client.py`, `test_semantic_client.py`, `typed_interop.py`; `transport/tests/independent.rs` | Standard-library Python decoder, reducer and TLS client; six typed replay prefixes agree with Rust; real source activation, Unicode replacement, stable-item selection and exact retry without scenes or Presented; combined graphical-observer integration remains |
| CPU reference renderer | `client-desktop/tests/pixels.rs`, `cli.rs` | Nine primitives, analytic pixels, linear composition, transforms, clipping, surfaces, paths, glyphs, bounds and explicit failure |
| Guarded GTK mirror | `client-desktop/src/desktop.rs`, `tests/interaction.rs`; `fixtures/render/native_smoke.py` | Actual GTK frame callbacks and two-mirror TLS smoke; generic advertised actions, scalar text context guards, bounded frame ownership and local observer transforms; pure bounded causal typing queue accepted; native FIFO and visible preedit integration remain |
| Expanded controlled source | `host-broker/tests/corpus.rs`; `client-desktop/tests/corpus_reference.rs` | Unicode edits, checkbox, stable collections, clipped scrolling and fresh transient identities; two TLS replicas equal direct source records and selected vector renders; absent font provider stays explicitly Partial and no Presented is claimed |
| Pinned source font helper | `fixtures/render/native_font.py`, `test_native_font.py` | Private pinned font configuration, bounded actual Pango positions and RVP1 outlines; bounded host closures retain authorized glyph bodies across publication owners; paged real TLS delivery accepted, combined native mirror gate remains |
| Pinned vector cursor | `client-desktop/tests/cursor.rs`, `spec/cursor-profile.md` | Explicit optional profile, bundled vector shapes, authorized custom paths, hotspot and canvas clipping; native session integration remains |
| Causal typing queue | `client-desktop/tests/typing.rs` | Bounded FIFO, one in-flight edit, exact command result plus source text/version/selection confirmation; native integration remains |
| Trusted resource admission | `protocol-core/tests/receiver.rs` | Authenticated producer helper validates the complete graph before granting its declared closure; invalid graphs and allocation failures roll back grants; strict default receiver unchanged |
| Actual GTK capture and SDK | `adapters/sdk/tests/gtk_capture.rs`, Python producer tests, `fixtures/gtk/measure.py` | Twenty strict publications: seventeen mapped snapshots and three lifecycle signals; typed scalar text/selection, collection and focus clocks, unmap/remap pin release, fresh identities after destruction, reentrant lifecycle ordering and bounded artifact decoding |
| Guarded native GTK commands | `adapters/sdk/tests/native_trace.rs`, `fixtures/gtk/native_check.py` | Common-ledger admission, live native identity/clocks/modality/lease guards, causal capture, exact retries and unknown-outcome retention; 32 dispatches/37 callbacks, 41 source snapshots and 261 hash-bound vector artifacts; bounded semantic fixture, no TLS/IME claim |
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
- **M1 remains open.** Complete all controlled command families, post-auth transcript
  integration, independent result interoperability and the full core-case audit.
- **M2 remains open.** The expanded source corpus and guarded desktop mirror are
  accepted separately. Bounded source glyph closures and network delivery are
  accepted. Complete rapid native typing,
  authorized resize, retained reconnect and complete source/reference/two-mirror
  comparisons must work together across the full interaction corpus.
- **M3 remains open.** Typed capture, explicit popup lifecycle, guarded native
  semantic dispatch and causal action evidence have passed review. Wire IME,
  persistent source restart, complete unsupported-rendering/fallback evidence
  and coherent whole-view transients remain. The declared seventeen-frame local
  visual comparison has passed.
- **M4 remains open.** Implement and test the explicitly supported isolated session
  shell, permissioned libei input, geometry/modality/focus/scaling, takeover,
  observed accessibility reconciliation and enforceable scope.
- **M5 remains open.** Combine the accepted semantic-client stable-item scenario with graphical mirrors; complete network fault and
  recovery traces, sustained resource churn, case-by-case acceptance audit and
  integrated named-hardware CPU/latency/bandwidth/memory measurements.

Current transport foundation limits and trusted boundaries are in its
[README](../transport/README.md). The CPU renderer's native allocation policy,
local image artifacts and pinned sampling are specified in
[its profile](../spec/renderer-profile.md). Native GTK antialiasing differences
are covered by an explicit calibrated tolerance and independent glyph checks;
this does not extend acceptance beyond the measured constrained corpus.
