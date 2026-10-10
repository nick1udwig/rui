# Implementation acceptance

RFC milestones **M0–M5 are accepted for the declared corpus and isolated backend**.
The [case audit](CASES.md) maps all 79 original cases exactly once: 77 P0 cases and both P1 measurements.
There are no remaining gates in this authorized scope.
RFC section 14 remains deferred.
The original `conformance.yaml` is preserved as a historical design artifact; this record and the case audit describe the implemented system.

Acceptance covers the pinned constrained GTK application corpus and the owned EIS session backend.
It does not establish arbitrary application, theme, compositor or whole-desktop support.
The measured CPU/native render paths miss the provisional 60 Hz budget; that limitation is reported explicitly under RFC section 13, alongside passing queue and memory ceilings.

## Milestones

| Milestone | Accepted scope and evidence |
|---|---|
| M0 — Feasibility and vocabulary | [GTK coverage](../docs/gtk-coverage.md) identifies the actual same-node snapshot boundary, cooperating stable semantics, seventeen mapped frames per style, 104 default-style gaps and zero constrained-style gaps. Unsupported texture/Cairo/effect regions are named; exported resources remain vectors. Decorations and compositor output are outside capture scope. |
| M1 — Pure protocol core | All 46 model-layer cases have executable negotiation, atomic graph/publication/resource, recovery, command and security evidence. Exact transcripts, generated faults, an independent Python decoder/reducer and mismatch shrinker accompany the Rust core. Required unknown variants and malformed/over-budget input fail explicitly. |
| M2 — Controlled vertical slice | Resource-bearing source and two actual TLS/GTK mirrors converge in records and RGBA after controls, list/scroll, popup/modal/tooltip, ordered canvas input, source resize, cache loss and retained reconnect. Observer transforms preserve source state. The controller runs through a constrained 32 KiB/s relay. [Reproduction and scope](../docs/desktop-controls.md). |
| M3 — Real GTK producer | Same-tree vector capture, typed semantics and bindings, guarded native actions, coherent whole-view capture, lifecycle fences, broker-preserving source restart and actual client IM composition pass. Two mirrors compare against authoritative source records and pixels after their own GTK paint callbacks. Local preedit stays private; committed Unicode edits have exact causal source evidence. [Native coverage and evidence](../docs/gtk-coverage.md). |
| M4 — Session integration | Actual EIS input, host XKB/repeat, unequal coordinate spaces, modality, focus, cursor and topology pass for the isolated exported session. Direct/remote click equivalence, stale routing after move/occlusion/modal, authenticated app-only denial, old-generation rejection and four held-input cleanup causes pass actual native runs. AT-SPI reconciliation remains explicitly OBSERVED. [Backend scope](../spec/session-shell-profile.md). |
| M5 — Hardening and translation seam | Actual TLS boundary cuts, exact retries, sustained resource churn, bounded slow readers and independent full-record/vector replay pass. A Python semantic-only client activates a button, replaces Unicode text and selects a stable collection item with exactly three effects and no Presented messages; both native mirrors converge. [Named-hardware performance measurements](../docs/performance.md) include CPU, latency, bytes, cache reuse and allocation peaks. |

## Final composition acceptance

[The canonical root native run](../fixtures/gtk/desktop-composition-interaction.json) uses separate authenticated Xvfb displays, a service-free private bus, pinned GTK 4.22.4 and the isolated test's simple IM.
Actual client GTK callbacks produce five Begins, twelve Updates, four Commits and one Cancel through the common TLS command ledger.
ASCII `a`, Unicode `é` and the native `x`/`y` batch produce `aéxy` with scalar caret 4.
The batch proves ordered native effects; queued overlap before a round trip is covered separately by the portable composition queue test.

Ten actual after-paint comparisons match complete source records and viewport pixels across controller and observer.
Five native captures check 935 glyphs with zero bad interior pixels under the declared calibrated tolerance.
The source owner returns to zero tracked bytes.
Preedit changes neither authoritative TextModel nor the observer's visible composition.
Escape cancels without editing; priority reset leaves an observer without automatically reacquiring control.

The negotiated optional `text.clock.gtk-editable4224.v1` profile describes the measured native GtkEditable clocks.
Commit confirmation requires the exact expected text/caret/clocks and the matching Applied cause on the coherent focused Surface, followed by actual current presentation before the next stage.
Generic sources retain strict clock behavior.
Ambiguous commands keep their original encoded bytes.
Fresh grants retire old local editing state; delayed revocations must match the current lease ID and generation.
Native reset/full-channel regressions preserve cleanup for the original input owner.

The selected clock profile is observed directly from the negotiated client capabilities; credential handshakes are excluded from post-authentication traces.
The original controller trace ends before priority reset, whose fresh scope and control state are checked in the native report.

The independent retained controller audit validates 275 packets, 296,069 exact packet bytes, six publications, 38 vector transfers and 23 commands: one Focus and 22 TextComposition stages.
The observer audit validates 212 packets and six publications.
Both replay all six record kinds and six exercised scene primitives, with no incomplete transfers or unrequested resources.
The retained 50-file corpus is 3,301,599 bytes and contains exact packets, capture envelopes and vector bodies.
Local raster comparison artifacts are not network payloads.

TXT-002 and TXT-003 combine this desktop evidence with the [fifteen native producer scenarios](../fixtures/gtk/composition-interaction.json): commit/cancel, focus and selection changes, removal, revocation, disconnect, renewal/expiry, and measured empty/no-op/deletion/insertion clock cases.
Source invalidation rejects late stages.
Portable queue, native reset and lease-transition regressions cover bounded admission, stale ownership and exact-command retention.

## Final regression record

The [machine-readable final verification](final-verification.json) records source and evidence fingerprints, commands, results and log hashes.
Independent root review required and verified corrections to stale input ownership, delayed lease notifications, native reset cleanup, causal text clocks, authorization races, allocation lifetimes and reconnect presentation matching.

- The final production workspace passes 412 Rust tests with three native-only tests omitted by default, strict workspace Clippy and formatting.
  The newly retained desktop composition corpus passes its additional default SDK test.
- The optional producer SDK suite passes 49 tests, including that retained test, with strict all-target Clippy.
  The desktop GTK/source feature suite passes 93 tests, with one native-only load test omitted by default, and strict Clippy.
- Native session validation passes 12 ordinary tests and all 33 explicitly enabled native tests, with strict all-target Clippy.
  Actual final baseline and held-input local takeover pass after the desktop lease changes; both return tracked owner memory to zero.
  The corrected comparison harness accepts only the current authorized scope paired with its own painted generation.
- The two source-font tests and native source-load test omitted by default pass explicitly in the pinned environment.
  The actual native IM probe passes Unicode, fallback typing, context reset and full-channel cleanup checks.
- All 88 Python tests, eight retained exact network audits and independent generated/full-field replay pass as recorded in the final verification.
  The unchanged independent corpus includes 1,275 semantic prefixes, 144 scene fixtures, six typed prefixes and 124 native prefixes across nineteen scopes.
- Fresh native Unicode, broker-preserving SIGKILL/restart and unsupported texture/Cairo/effect regressions also pass after producer composition integration; four fresh Rust replay tests and independent 23-prefix/five-scope replay agree.
  [Root evidence](../fixtures/gtk/final-native-root-verification.json) records the callback, exporter-trap and disposal checks.

CI replays the retained native evidence without requiring a desktop.
Actual GTK, EIS and accessibility probes require the documented pinned native environment.
GTK after-paint is a presentation opportunity, not a physical scanout measurement.

## Limits retained in acceptance

The strict profile carries no raster fallback, business logic or executable remote components.
Unsupported content stays explicit Partial/unavailable.
AT-SPI observations cannot establish trusted native input association from titles or geometry.
Host IME is not provided by the isolated physical-input backend; the GTK producer uses negotiated client-owned composition.
Memory ownership and queue bounds are enforced independently of the provisional latency targets.
Current supported profiles and trust boundaries are documented in the [transport README](../transport/README.md), [renderer profile](../spec/renderer-profile.md), [input profile](../spec/input-profile.md) and [case audit](CASES.md).
