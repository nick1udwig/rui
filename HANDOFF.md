# RUI implementation handoff

This is a historical handoff from 2026-10-02, before M0–M5 acceptance and the browser-client extension.
Its paused agents, open gates and dirty-work descriptions record that point in time.
Current product scope is in [status and next steps](docs/status.md); the [implementation log](IMPLEMENTATION.md) and [acceptance record](conformance/STATUS.md) retain milestone evidence.

Recorded 2026-10-02, approximately 21:15 UTC / 14:15 America/Los_Angeles.
Implementation HEAD before this handoff: `b5b8266` on `master`.
The working tree contains substantial intentional, uncommitted implementation.
Do not reset, clean, or blanket-stage it.

## User direction and working mode

The user requested a project-manager workflow: understand the project, delegate implementation to **GPT-6.1 Sol at xhigh**, review their work as final arbiter, require corrections, and commit in logical increments.
The implementation scope is RFC **M0–M5, all 79 conformance cases, including P1 measurements**.
RFC section 14 deferred features remain excluded.
Do not mistake component tests or matching test names for completed integration gates.

The latest request interrupted implementation to dump status into this document.
All three agents are now paused at safe boundaries; no agent commands or tests remain running.
Resume the same workflow when implementation resumes; this handoff does not claim the objectives are complete.

Read `RFC.md`, `IMPLEMENTATION.md`, `conformance/STATUS.md`, `spec/rui.proto` and `spec/session.proto`.
The root `conformance.yaml` is a historical specification; its `specified_not_implemented` entries were deliberately not rewritten.
`IMPLEMENTATION.md` has some older remaining-work descriptions; the distinctions between committed and dirty work below are more current.

## Milestones

| Milestone | Acceptance |
|---|---|
| M0 | Accepted only for the declared pinned GTK corpus and capture boundary |
| M1 | Open: host integration of all command families, post-auth transcripts, full case audit |
| M2 | Open: expanded authoritative fixture, two real mirrors, resources/reconnect and comparisons |
| M3 | Open: typed GTK slice under review, guarded remote callbacks, IME, coherent transient composition |
| M4 | Open: isolated session shell, permissioned libei, scope enforcement, observed AT-SPI reconciliation |
| M5 | Open: complete semantic-client scenario, network fault/churn evidence, audit and integrated measurements |

## Commits already accepted

```
bf84f39 docs: record RUI protocol design and conformance specification
e4594f7 feat(core): implement bounded replication and session authority
862bdfa test(conformance): add independent replay oracle and CI gates
7c6569a feat(scene): complete bounded primitive vocabulary and independent fixtures
c799920 feat(receiver): coordinate bounded publications and resource lifecycles
2cb6559 feat(host): add controlled two-client authority and privacy fixture
802aeff feat(glyphs): negotiate verified single-outline glyph mapping
9067efd test(core): exercise generated replay and bounded mutation fuzzing
79b4546 feat(core): add bounded input interpretation and scoped transcripts
6a98640 feat(transport): bind authenticated views to pinned TLS WebSockets
9a92494 feat(gtk): capture submitted vector trees with bounded producer replay
8422c7a feat(renderer): render bounded vector scenes with pinned CPU reference
4aff589 feat(commands): share bounded admission across all command families
e2f1ad6 test(recovery): exercise every chunk cut and sustained resource churn
e96c766 test(gtk): gate native replay pixels and decoded glyph geometry
053f411 test(conformance): minimize independent replay counterexamples
b5eac44 docs: track reviewed command recovery and GTK comparison evidence
b5b8266 feat(session): grant semantic-only control without visual presentation
```

## Accepted architecture and important boundaries

- Workspace crates: `protocol-core`, `host-broker`, `transport`, `client-desktop`, `adapters/sdk` (package name **rui-producer-sdk**).
- Core: bounded protobuf preflight, atomic typed graph reducer, retirement, resource authorization/cache/pins, publication assembly and shared memory accounting.
  `ViewReceiver` separates Applied from trusted Presented.
- All four command families share one exact-byte, family-bound, contiguous producer sequence ledger.
  A reserved ambiguous outcome survives callback panic; exact retries never redispatch.
  `submit_controlled_with_session` validates current session authority and then lends the trusted dispatch callback the session broker for coherent publication and truthful source evidence.
- `InputRouter` and `CompositionMachine` are bounded pure primitives, not yet fully connected to the host.
  Held input is conservatively owned before external callbacks; cleanup cannot depend on a fresh route.
  Composition is transient, scalar-indexed and invalidated by editing-context changes.
- `TranscriptRecorder` is scoped, metadata-only by default, optionally retains authorized exact bytes, shares accounting and purges on revocation.
  It is not yet connected to the network host.
- The current controlled host is still the original counter/private Unicode text fixture.
  Read permission is independent of control.
  Observer redaction removes private content, lengths and glyphs.
  The expanded fixture has not been built.
- Transport uses pinned rustls TLS **plus ordinary certificate/hostname validation**, explicit local pairing, immutable approval binding, bounded HTTP upgrade, binary uncompressed WebSockets and recursive schema preflight.
  Source publication chunks borrow budgeted payloads.
  Revocation races and cleanup ordering/panic behavior were reviewed and corrected.
- Tungstenite can trace plaintext.
  The transport intentionally disables the entire `log` facade using `max_level_off` and `release_max_level_off`; this global Cargo feature-unification tradeoff is documented.
  Packet/ClientHello Debug is redacted and transient credential copies are zeroized.
- Committed transport limitations: source core envelopes must still fit the negotiated envelope limit; fixture server has no resource closure; raw/IME/ resize source dispatch is not complete; fresh clients reject resume without a retained replica.
  Do not claim these are completed by the existence of schema.
- Optional `semantics.only.v1` selects `fixture` / `semantic-only` OpenView.
  Projections omit Scene/Binding, visual bindings, cursor and resources; Surface coverage is Unsupported.
  Kind **35 SemanticReady**, with RevisionAck payload, follows Applied and enables only semantic commands.
  Presented is rejected for this scope.
  Raw input/composition/resize cannot use semantic readiness.
  Mirror views still require actual renderer completion.
- CPU renderer: Cairo 1.18.4 geometry only; Rust 2x2 sampling and linear premultiplied composition, all nine primitives, transforms/clips/surfaces, bounded geometry/work/pixels and explicit unsupported behavior.
  Native Cairo allocation estimates are conservative accounting, not measured process RSS.
- Glyph profile `glyph.outline-single.v1`: RVP1 outline normalized to one em, y-down baseline; glyph IDs are zero; source supplies shaped size/positions.
  Repeated glyph instances compose in order before node opacity.
  No silent font substitution or client reshaping.
- GTK capture snapshots the actual child GSK node in the cooperating snapshot vfunc and appends that **same node** for local rendering.
  Native popup capture has explicit identity.
  Strict style removes named unsupported effects; no raster fallback is transmitted.
  Native PNG/BGRA images are local diagnostics.
- Popup artifacts currently carry a topology-only PARTIAL parent.
  Selected-scene reference rendering is valid; whole-view completeness is not established.

## Active agents and dirty work ownership

All three implementation agents use the requested GPT-6.1 Sol xhigh mode and share this checkout.
Root owns review, acceptance, commits, CI/lock coordination and the independent Python client.
Use existing agents with `followup_task` when resuming, or recreate equivalent explicitly delegated agents if unavailable.

### `/root/authority_commands`

Paused; no tools/tests running at its status report.
The only dirty owned files:

- `transport/src/client.rs`
- `transport/tests/loopback.rs`
- `transport/README.md`

This frozen slice makes `NetworkClient::receive` cancellation-safe for receiver mutation and returned events: required replies are queued synchronously, retained until flush succeeds, and flushed before receive/send.
Queue limit is eight packets plus negotiated outbound bytes.
Cancellation can resend an idempotent acknowledgement.
Agent reports **7 real TLS loopback tests, strict all-target transport clippy and fmt pass**, including cancelled chunk reception, retained AppliedAck, presentation/control and once-only command behavior.
**Root has not accepted or committed this slice yet.**
Review cancellation at Sink send/flush boundaries, retained replies and allocation/accounting before accepting.

Next owned work: expanded controls/text/selection/checkbox/list/reorder/scroll/ popup/modal/tooltip/vector-canvas fixture, authorized resize; raw/IME routing and cleanup through the common ledger; native source-font provider integration; resource-bearing producer and authorized closure; publications larger than an envelope; retained reconnect; post-auth scoped transcripts.
Coordinate a trusted source-producer interface with the GTK agent.
No expanded corpus implementation exists yet.

### `/root/model_wire`

Dirty desktop implementation owns `client-desktop/Cargo.toml`, `src/lib.rs`, `src/renderer.rs`, new `src/interaction.rs`, `tests/interaction.rs`, `src/desktop.rs`, `src/desktop/gtk.rs`, and `src/bin/rui-mirror.rs`.
Root owns the corresponding dirty `Cargo.lock` integration.

The optional `desktop` feature adds exact gtk4 0.9.7, transport, Tokio and prost; `rui-mirror` requires that feature.
It uses a Tokio connection worker, bounded commands/events, replaceable frames and watched local viewport.
CPU RGBA pixels remain budget-owned through GLib Bytes / GdkMemoryTexture.
GTK after-paint is a submission/presentation opportunity, **not proof of physical scanout**.
Generic interaction uses published Binding regions in surface-content coordinates and advertised actions, without fixture IDs or business logic.
Observer pan/zoom/ resize remain local.
Four interaction/RGBA ownership tests passed before the last Focus/renewal fixes.
Final agent check `cargo check -p rui-client-desktop --features desktop --offline --locked -j 1` passed and package formatting completed.
Full feature tests/clippy and a live GUI smoke test have not run; this slice is unaccepted.

Root review findings sent to this agent, still requiring verification:

1. Fresh unique acquisition IDs and lease renewal were implemented before pause; verify behavior with tests, not just the passing feature check.
2. Generic advertised Focus dispatch was implemented before pause; verify native and host focus behavior with tests.
3. Local IM preedit must be bound to the source target, text version, focus generation and lease.
   Reset on changes; late commits must not enter a newly selected context.
   Plain ReplaceText alone does not prove composition support.
4. The worker selects on `receive`, so the transport cancellation fix is a real dependency, not merely an optimization.
5. Critical Clear/Quit delivery can still be lost when the bounded UI channel is full.
   Fix this before accepting lifecycle/disconnect behavior.

New untracked `fixtures/render/native_font.py` and `fixtures/render/fonts/{LiberationSans-Regular.ttf,LICENSE}` are source-native font-provider groundwork owned by this track.
Provider pins Pango/Cairo/font and uses process-private Fontconfig, returning vectors and source-shaped glyph positions, never wire font or raster data.
Agent smoke test shaped `AéΩ אב` into five source outline glyphs.
Font SHA-256 in the provider is `baccc64becc3eb7d104b7c84d99f5314a0a1f896e2b3ea6c2f22fc08d2003bee`.

Queued next assignment after GUI/M2 comparison: isolated `rui-session-shell` and permissioned libei/EIS backend, with real granted-region mapping and M4 geometry/modality/focus/scaling/takeover tests.

### `/root/receiver`

Typed GTK slice is dirty and has not been accepted.
Files:

- `adapters/sdk/{producer.py,test_producer.py,src/lib.rs,tests/gtk_capture.rs}`
- `adapters/gtk4/{capture.py,test_capture.py,state.py}` (`state.py` new)
- `fixtures/gtk/{corpus.py,golden/,measurement.json}` including new golden frames

Implemented: typed TextModel/content clocks, native backward scalar selection, CollectionModel selection and partial/full materialization, per-view native focus SessionState, explicit popup map/unmap/destroy, incarnation/object guards, and bounded 4 MiB model-clock cache.
Final isolated run **`/tmp/rui-gtk-rich5`** completed: **20 publications per style** (15 main snapshots, two popup snapshots, two popup unmaps and one popup destroy), **104 default-style gap reports, zero strict gaps**.
Golden artifacts now exactly match rich5: 20 envelopes/manifests plus 32 unchanged vector bodies.
Backward selection is `[19,15]`; its content clock stays 2.
Five SDK Rust tests, nine Python tests, SDK all-target strict clippy and diff check pass.
No processes remain running.
A prior 17-frame receiver replay released memory to zero on close.

Accepted docs and pixel-comparison report still describe the old ten-frame corpus: **do not commit regenerated fixtures without updating/revalidating their evidence**.
Strengthen typed clock/lifecycle Rust assertions and rerun all **17 mapped** native-versus-renderer frames.
Native backward selection uses GtkText `move-cursor`; backward GtkEditable `select_region` clamped to a caret on this pinned build.

No remote source-command bridge has started.
Next: review/commit typed slice, rerun all mapped pixel comparisons, then real Rust ledger admission → trusted pipe → serialized native GTK identity/context checks → actual application callbacks → causal SourceEvidence.
Local scripted actions are not proof of remote dispatch.
IME and coherent multi-surface composition remain.
Afterward, own controlled AT-SPI observed reconciliation, event gaps/crawl races/restarts, explicit bus/source/object identity and isolated bounded-time blocking calls.

### Root-owned uncommitted independent-client slice

Files: `conformance/{README.md,semantic_client.py,test_semantic_client.py, typed_interop.py,golden/typed.jsonl}`, `transport/tests/independent.rs`, `protocol-core/examples/replay.rs`, and two new Python CI steps in `.github/workflows/core.yml`.

The Python standard-library implementation uses independent protobuf decoding, normal TLS validation plus exact pin, bounded WebSocket framing, exact publication digest/count checks, semantic/text/topology validation, stable IDs and typed clocks.
It discovers actions/text/collections, sends Applied then SemanticReady, and never sends Presented.
CLI supports activation, scalar text replacement, stable-item selection and exact retry; the last scenario is not yet exercised against an expanded host.
It is a bounded conformance client, not a production Python heap-accounted receiver or retained-resume client.

Validation completed:

- **11 Python client tests + 2 shrinker tests pass** after review fixes.
- **6 typed exact-wire prefixes** match normalized Rust/Python IDs, values, Unicode ranges, clocks, actions/states, collection ordering/count/selection, retirement and admission.
  `replay --full-state` is a new fixture-only option.
- Actual isolated TLS test invokes Python CLI: Increment exact retry has one source effect, Unicode replacement converges, two consumed commands and zero Presented acknowledgements.
  Passed twice before the last tiny validation additions.
  Test subprocess has a 20-second kill deadline.
- Targeted Rust clippy for `replay` and `independent` passed.

GTK agent reviewed this code.
Root corrected exact typed transaction clocks, new collection clock, text selectable/availability rules, collection child ownership, semantic relation kinds, surface root/focus ownership, required surface metadata, source token evidence, redacted-node private fields, focus-generation command preconditions and recursive optional-metadata handling.

**Known next root fix identified immediately before interruption:** Python `SemanticReplica` still uses fixed 4096-record and 128-depth bounds rather than the possibly lower **negotiated** bounds in `SemanticClient.limits`.
Make those replica limits configurable and propagate negotiated records/depth after hello; test lower limits.
Byte/chunk/publication limits already use negotiated values.
Also consider canonical comparison of explicitly encoded default scalar values; current `known_record` ignores optional metadata recursively but remains stricter than protobuf semantic equality for alternate default-value encodings.
Document or correct that conservative limitation.
Run final TLS test after these changes, then review/commit this slice separately from other agents' dirty work.

## Accepted tests, artifacts and reproducibility

- Independent baseline: `python3 conformance/check.py` last passed **1,275 semantic prefixes, 16 seeds, 144 scene fixtures**.
- Core hardening: 2,048 generated prefixes, 10,000 malformed mutations.
- Root recovery matrix: every publication/resource chunk cut, duplicate chunks, disconnect/retry/cache loss/fresh-state equivalence; 256 resources churned.
  Its command/presentation callbacks are synthetic, not source/network evidence.
- Common ledger acceptance: 159 core tests and 11 host tests at that commit.
- Semantic-only commit: 18 command, 25 session, 11 host and 20 transport tests including independent Python TLS at that stage.
  Later test counts differ.
- Renderer accepted baseline: 26 tests; passes against both system Cairo 1.18.4 and freshly built checksum-pinned minimal Cairo.
- Accepted GTK comparison: ten strict frames, zero interior mismatches/alpha error, 942 native glyph outline bounds within 1.56e-15 pixels.
  Edge max RGB error 127 main / 114 popup.
  Initial viewport mean threshold 3 failed popup at 3.122; explicitly calibrated to 4 plus edge-mean ≤8, retaining tight interiors and actual decoded glyph omission/shift/size negative controls.
  See committed `fixtures/gtk/pixel-comparison.json` and `docs/gtk-coverage.md`.
- Accepted source run `/tmp/rui-gtk-release4/{default,strict}`; main visual review `/tmp/rui-gtk-main-review.png`.
  Root inspected actual rendered controls/text/ list/canvas/menu positions.
  New rich5 supersedes source data only after review.
- Reference CLI: `cargo run -p rui-client-desktop --example render_capture -- CAPTURE.json OUT.pam --scene`.
  `--scene` is selected complete local-scene evidence, not whole-view acceptance.

Useful commands, after checking agents are not editing the same slice:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
python3 conformance/check.py
python3 -m unittest adapters.sdk.test_producer
python3 -m unittest fixtures.gtk.test_compare adapters.gtk4.test_capture
python3 -m unittest conformance.test_semantic_client conformance.test_shrink
python3 -m conformance.typed_interop
cargo test -p rui-transport --test independent
cargo test -p rui-transport --test loopback
cargo build -p rui-client-desktop --features desktop --bin rui-mirror --locked
```

Do not claim the full current dirty workspace passes; focused checks were run per slice.
Optional desktop checks are not yet part of accepted default CI.

## Environment and operational notes

- Workspace `/home/nick/Work/git/rui`; writable roots workspace and `/tmp`.
  `.git` is read-only in the sandbox.
  Git add/commit uses escalated execution with `prefix_rule: ["git"]`; the user already authorized logical commits.
- Local loopback tests need escalated network permission.
  Dependency fetches have been approved; `cargo fetch --locked` most recently fetched futures-macro 0.3.34 for GTK.
  No outstanding dependency download blocker was known.
- Rust 1.98.1, protoc 35.1, GTK 4.22.4, Cairo 1.18.4, Pango 1.58.2, GLib 2.88.3, libei/libeis 1.6.0, libportal 0.10.0, AT-SPI 2.60.6, Python 3.14.7, PyGObject 3.56.3.
  Machine CPU: Intel i7-1280P.
- `mise` may warn about unrelated oh-my-pi version lookup; do not alter system configuration to silence it.
- No personal display is available.
  Isolated Xvfb binary: `/tmp/rui-xvfb/usr/bin/Xvfb`.
  Broadway triggered a native GTK popup assertion; diagnosed earlier, not the accepted gate.
  Do not inject into a personal desktop.
- M4 may use an explicitly granted isolated EIS fd/session.
  Installed headers `/usr/include/libei-1.0/{libei,libeis,liboeffis}.h`; relevant APIs include `eis_setup_backend_fd`, `eis_backend_fd_add_client`, `ei_setup_backend_fd`.
  Prove mapping through granted logical regions, not framebuffer pixel guesses.
- CI builds minimal Cairo 1.18.4 via `ci/build-cairo.sh`; release SHA-256 `445ed8208a6e4823de1226a74ca319d3600e83f6369f99b14265006599c32ccb`.
  Local build artifacts `/tmp/rui-cairo-prefix`, `/tmp/rui-cairo-buildcheck`, `/tmp/rui-ci-venv`; `LD_LIBRARY_PATH=/tmp/rui-cairo-prefix/lib` was used for renderer tests.
  This minimal build is not a replacement for full GTK-native font/render dependencies; do not use it blindly for GUI runs.
- Never reuse one Cargo target directory for different checkout paths: stale artifacts already caused misleading review once.
  Use a distinct target for each isolated checkout; root workspace uses its normal `target/`.
- Preliminary core-only release benchmark exists in `protocol-core/examples/measure_core.rs`: 10k semantic records, snapshot 398,769 bytes; one-record update 45–46 bytes; 20-record update 824–846 bytes.
  Earlier timings were collected during development and are **not final P1 evidence**.
  Rerun idle with integrated render/network/client-count/churn and named hardware, including allocation/queue counters and distribution tails.

## Resume order

1. Reconcile final agent freeze reports and `git status`; preserve every dirty file.
   Read this handoff and the relevant contracts before editing.
2. Review and commit cancellation-safe transport as its own slice.
3. Finish negotiated Python limits/default-encoding check, final interop tests, and commit the independent client/typed vectors/CI separately.
4. Review typed GTK rich5 fixtures, clocks, lifecycle and memory evidence; refresh pixel report/docs to match actual golden bytes before committing.
5. Finish and review GUI build, focus/IME/lease fixes, two-mirror runtime evidence and source/reference comparison.
   Do not fake presentation acknowledgements.
6. Continue expanded source/transport integration, then actual GTK remote bridge.
7. Implement isolated session-shell/libei and observed AT-SPI tracks with explicit supported-backend scope; prove raw cleanup, routing, modality and takeover.
8. Complete semantic stable-item + independent graphical observer scenario, network reconnect/resource faults, sustained multi-client churn, complete all-79 case audit and integrated P1 report.
   Mark milestones complete only when their full executable gates pass, then leave logical commits and a clean tree.
