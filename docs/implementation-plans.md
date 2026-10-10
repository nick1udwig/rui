# RUI implementation plans

These plans extend RUI from its accepted controlled prototype toward a faster host, broader source coverage, reusable clients, and a semantic UI that can use a different layout from the host. They define implementation steps and completion evidence for the next developer. They do not mark any of this future work complete.

The starting code is `5272eda` on `master`, reviewed on 2026-10-10. RFC milestones M0–M5 and all 79 original cases are accepted for the declared constrained GTK corpus and isolated session backend. The [acceptance record](../conformance/STATUS.md) remains the record of that scope. The four workstreams below add capabilities and performance goals beyond it.

## Delivery order and shared constraints

Start with performance measurement and browser CI so later changes have a reproducible baseline. Remove the demo polling delay before attempting renderer or worker redesign. Establish valid incremental GTK publications before optimizing how clients copy and draw those updates. Begin the semantic UI after the portable session supports semantic-only negotiation and readiness; it does not need to wait for complete browser input parity or general desktop support.

| Workstream | First deliverable | Main dependencies | Completion boundary |
| --- | --- | --- | --- |
| Performance | Stage timings for the demo and existing load fixture | Existing host and both renderers | Bounded event-driven publication, valid deltas, measured latency and rendering improvements |
| Broader application coverage | Coverage inventory and a second owned application | Existing capture and source identity contracts | Explicitly supported applications, styles and backends with source, interaction and visual evidence |
| Browser and library maturity | Reproducible WASM and browser CI | Existing Rust/WASM and TypeScript packages | Shared session behavior, stable integration contracts, tested capability subsets and distributable packages |
| Semantic presentation | Semantic-only portable session and read-only DOM tree | Configurable negotiation, complete typed semantics | Generic responsive controls that issue guarded semantic commands and converge with the host |

Performance instrumentation, coverage measurement and CI setup can proceed independently. Changes to publication versions, source identity, common session behavior or the TypeScript state API need a single agreed contract before multiple consumers change.

The first implementation series should be P1 and L1, followed by P2 and P3. Use P1 results to select P4 optimizations. C1 precedes additional coverage claims; C3 supplies the second reusable application needed by the semantic gates. S1 needs configurable session/readiness behavior from L2/L3, S2 needs L3's complete semantic types, and S3 needs the semantic-command subset of L4. Full native-actor migration, raw keyboard input and additional compositor support need not block the first semantic UI.

Preserve these invariants throughout all four plans:

- The host owns application state, source focus, selection and layout. A client's viewport, navigation focus and unsubmitted draft are local state.
- Rust remains the authoritative wire validator and replica reducer for the native and WASM clients. Derived JavaScript render or DOM state must be recoverable from the committed Rust state.
- Every command passes through the existing authorization and exact-byte command ledger. Retries preserve the original bytes, sequence, view and authority; they never recreate an ambiguous edit under a fresh lease.
- Applied, visual Presented and SemanticReady are distinct. A renderer reports an actual presentation opportunity for the current connection and revision. A semantic-only client never fabricates Presented to obtain control.
- Resource authorization, source guards, view/epoch identity, record retirement and independent observer projections survive optimization. Source callbacks and final native dispatch must still recheck their own context.
- The strict profile transmits vectors and authorized semantics. Unsupported content remains explicitly partial or unavailable. Rasterizing a missing effect, glyph or image into wire geometry is not an allowed fallback.
- Queue, history, resource, renderer and working-memory limits remain enforced. Account for native allocations, JavaScript copies and socket buffering separately from Rust reservations. Optimize within explicit limits rather than silently increasing them.
- Native automation runs on owned displays, buses and input sessions. Retained diagnostic artifacts exclude credentials and unauthorized text; exact packet recording remains an explicit fixture facility.

Implement each numbered step as a reviewable change or small series: behavior, relevant regression evidence, and documentation together. Keep existing acceptance artifacts as historical evidence; add fresh reports with source hashes and environment details when behavior or measurements change. Update [IMPLEMENTATION.md](../IMPLEMENTATION.md) as steps complete, including any capability that remains intentionally unsupported.

## Performance

### Starting point

The manual host in [rui-demo.rs](../client-desktop/src/bin/rui-demo.rs) checks local changes every 250 ms. [NativeDispatcher](../adapters/gtk4/native.py) avoids idle captures through `capture-if-changed`, but the changed state must still wait for that check. [GtkCommandHost](../adapters/sdk/src/gtk_host.rs) currently projects and publishes snapshots, retaining one replacement for a slow view until its readiness permits installation.

The core already supports transactions and bounded retained history. The existing controlled host and [integrated benchmark](../transport/examples/measure_integrated.rs) exercise small updates and cached resources. The GTK adapter needs to use those facilities correctly; a new network protocol is not the first task.

The browser currently copies the full committed state through WASM after an applied publication. Its [renderer](../client-browser/src/renderer.ts) caches vector paths and coverage masks but still performs frame composition. The native [reference renderer](../client-desktop/src/renderer.rs) and the browser renderer have different coverage contracts. Keep their measurements separate. The small interactive demo's draw timing does not establish the 10,000-record workload's performance; [docs/performance.md](performance.md) records substantial remaining costs in that workload.

### P1 Measure each stage

Add opt-in, bounded timing and count instrumentation at the actual work boundaries:

1. Local source callback or admitted command, dirty notification, capture scheduling, and coherent capture completion.
2. Projection/diff construction, encoding, resource closure construction, and per-client outbound queue residence.
3. Client receipt, resource waiting, replica application, WASM-to-JavaScript export and render preparation.
4. Geometry/coverage work, composition, output conversion, render submission and the following native/browser presentation opportunity.

Use existing revisions, command sequences and scoped diagnostic identifiers to correlate events. Measure durations with monotonic clocks within each process. For different machines, report local stage durations and client action-to-client presentation round trips; do not subtract unsynchronized host and client clocks.

Extend the browser harness to export structured results rather than relying on its footer. Separate steady-state rendering from cold initialization, resource downloads, fixture screenshots and pixel-comparison overhead. Keep correctness comparisons in a companion run when they materially distort timing. Use a performance runner with a bounded deadline sized for the workload; the current short integration harness is not sized for hundreds of baseline actions.

Measure these scenarios on named hardware in release builds:

| Scenario | Required observations |
| --- | --- |
| Small GTK demo, source-side edit and browser-side edit | At least 200 warm actions, p50/p95/p99, callback-to-publication delay, command result and causal presentation |
| Existing 10,000-record scene, one-record and twenty-record updates | Per-client application/render timings, bytes, resource reuse, full-state equivalence |
| Two healthy clients and one throttled or stopped reader | Queue high-water marks, closure behavior, effect on healthy-client latency |
| Cold connection, retained resume where supported, forced reset and cache loss | Time and bytes to readiness, peak ownership, pending-command outcome |
| Hidden/visible tab changes and repeated connect/dispose | Presentation suppression, control release, retained state and memory stabilization |

Keep browser/version, compiler, native libraries, viewport, scale, network conditions and workload size in every report. Use at least three runs for comparisons and report variability. Avoid concurrent builds or unrelated load during benchmark runs. Preserve the existing native load fixture so before/after results remain comparable.

Completion: a developer can identify which stage dominates each scenario from a retained report and reproduce the result. No latency claim may substitute Applied for a presentation opportunity.

### P2 Replace periodic source capture with dirty notifications

Add an adapter-owned notification path from the cooperating GTK process to the Rust source owner. Keep it separate from the ordered private request/reply stream unless that stream first gains an explicitly framed message dispatcher; an unsolicited dirty message must never be mistaken for a command reply.

Use a bounded notification carrying source incarnation and a monotonic dirty generation, backed by a level-triggered dirty state. Coalesce repeated wakeups while work is pending. A full notification channel must leave the source dirty so the last change is eventually captured. Stale notifications from a replaced source cannot schedule work for its successor.

Connect semantic callbacks, size changes and supported visual invalidation to that state. `Source.changed()` alone does not prove coverage of animation, style or visual-only changes. Arrange capture at a GTK frame boundary using the existing same-node capture contract. Bound work to one pending capture and retain a newer dirty generation if another change occurs during capture. Include a slow reconciliation mechanism only where the adapter cannot observe all changes, and document its maximum detection delay.

Have `GtkProducerPort` serialize notification-triggered capture with command execution on the source owner. Capture caused by a remote command must preserve that command's source evidence; a nearby local change must not be attributed to the command merely because it shares a frame. Command replies, capture artifacts and consumption acknowledgements retain their current ordering and lifetime rules.

Replace the 250 ms loop in `rui-demo` with notification handling, keeping startup, source-exit and shutdown supervision independent of dirty events. Do not move synchronous GTK or private-pipe work onto the network I/O task.

Completion: idle operation creates no publication churn; the final state of a burst is delivered; changes during capture are not lost; source restart and notification overflow are bounded; commands and local changes remain correctly attributed. The local-edit measurement no longer has an intentional 250 ms scheduling delay.

### P3 Publish incremental GTK updates

Introduce a bounded publication builder between coherent source capture and per-view transport publication. Reuse the current source capture format initially; reducing capture/file costs can follow measured evidence. The first change is from full wire snapshots on every edit to valid transactions.

1. Retain the previous authorized projection for each view, including the public-to-source identity and clock mapping needed for command guards.
2. Compare canonical record content and resource requirements. Generate puts/deletes for changes in all six record kinds, including child ordering, text/selection, topology, bindings, focus and resource references.
3. Enforce transaction clocks. The next publication revision is exactly the base plus one; record/model clocks follow the reducer's transition rules. Source captures can skip intermediate changes or carry reset-oriented clocks. Do not copy those numbers blindly into transaction versions. Preserve the mapping back to the actual source clocks and causal evidence used by native dispatch.
4. Validate the complete candidate through the existing reducer before publication. At each emitted step, applying the transaction and applying a snapshot of that candidate to copies of the same predecessor must agree on records, new retirements and required resources. For a whole sequence versus a fresh snapshot, compare live records and resources separately: a snapshot cannot reconstruct the history of objects created and deleted between its endpoints. Verify permanent retirement across the transaction sequence independently.
5. Attach the authorized resource closure through the existing broker APIs, including `publish_with_attachment`. Pin resources for as long as active publications/history need them and release them on acknowledgement, reset, revocation and disposal.
6. Separate total publication limits from per-message limits. The current GTK publication path also checks a whole encoded snapshot against `envelope_bytes`; review this restriction when integrating bounded chunk delivery. Enforce the negotiated snapshot/transaction and lifetime budgets for the whole publication, and the envelope ceiling for each transport message. Keep independent bounds on the private capture format and add a publication larger than one message to the tests.

A slow view may keep a bounded latest target state before encoding its next transaction. Once exact publication bytes have entered an outbound queue or retained history, never rewrite their base, payload or digest. If history, identity or resource continuity is unavailable, use the existing reset/snapshot path at a defined boundary. One slow observer must not stall source mutation or unboundedly retain intermediate captures for healthy clients.

Keep snapshot fallback for initial connection, source replacement and recovery. The current GTK adapter invalidates its view on disconnect; reconnecting it can require a fresh snapshot even though the general transport supports retained resume. Do not claim GTK retained-delta resume until its view lifetime, retained authorization, history and attachments have been implemented and fault-tested together.

Completion: a warm one-field edit sends a transaction with no unchanged scene tree or cached resource bodies. Tests cover coalesced source changes, multi-record text edits, removal/rematerialization, popup/modal transitions, changed projection permissions, source replacement, every transfer cut, cache loss and a stalled reader. Independent replay agrees with direct snapshot state.

### P4 Reduce application and render work

Use P1 results to choose one optimization at a time. Candidate changes, in recommended order, are:

- Export a generation-bound change set from Rust: base/new revision, changed and retired IDs, plus reset/scope markers. Supply a full state copy when the caller misses a generation. JavaScript may update a derived cache only after Rust commits; it does not become another wire reducer.
- Reuse verified resource and render preparation data across publications. Cache keys include scope or immutable content identity and all relevant geometry, transforms, clip, scale and paint inputs. Same IDs after a reset cannot reuse unrelated data.
- Reuse pixel/coverage buffers and avoid allocation per pixel. Separate geometry invalidation from color-only changes where the renderer contract permits it.
- Add damage tracking with a full-render comparison oracle. Damage includes old and new bounds, ancestor transforms/clips/opacity, overlapping siblings, surface stacking, removal and resources. Isolated group opacity and translucent overlap may require repainting the entire affected group.
- Move work to a worker or use OffscreenCanvas only after measuring a main-thread bottleneck. Define transferable ownership, generation cancellation and bounded queues first. Keep a supported main-thread path and avoid making shared memory or cross-origin isolation a requirement for the existing tailnet demo.

Maintain the native reference renderer's declared output contract and the browser renderer's separate coverage tolerance. Do not trade linear compositing, glyph positions or unsupported-content detection for speed. Optimize reset/cold behavior as well as warm updates; a cache is not allowed to grow with every revision.

Completion: optimized and full reference paths agree under their declared comparison rules, stale async work cannot paint or acknowledge a newer session, and the reports show the improvement at the targeted stage without shifting unbounded work elsewhere.

### P5 Establish performance gates

Adopt the following starting goals for the next performance milestone, then freeze the workload and measurement procedure before evaluating changes. These are future targets, not current results:

- Small warm loopback demo: action-to-presentation p95 at or below 100 ms and p99 at or below 150 ms for both local-host and client-originated edits.
- Small warm browser scene: render preparation, composition and submission p95 within 16.67 ms, measured separately from transport and the following presentation opportunity.
- Large fixture: preserve small one/twenty-record updates and zero warm resource-body retransmission; report per-client frame and end-to-end distributions. Keep 60 Hz as an explicit outstanding goal if the large workload still misses it.
- Every scenario: existing queue/memory ceilings hold, slow-client failure is isolated, and repeated lifecycle cycles stabilize rather than accumulate owners or retained copies.

Use generous correctness timeouts in ordinary CI. Enforce timing thresholds only on a named, controlled benchmark runner; save raw samples and compare against a fixed baseline. If a target proves infeasible, record the dominant cost and an explicit revised scope before changing the target. Do not infer a general 60 Hz or bandwidth claim from the small demo.

## Broader application coverage

### Starting point

[GTK coverage](gtk-coverage.md) currently establishes a pinned cooperating producer and constrained styles. The measured default-style corpus contains 104 gap reports; this count is historical evidence, not a count of unique missing features. The exporter handles a subset of GSK nodes, and source semantics/actions are explicitly registered.

[AT-SPI](../adapters/atspi/README.md) supplies bounded observed hierarchy data. It does not establish coherent visuals, native surface association or safe interactive control. [Session integration](../spec/session-shell-profile.md) establishes an isolated owned backend. None of these components alone establishes arbitrary live-desktop coverage.

### C1 Publish a capability and application matrix

Create a maintained matrix of application/version, toolkit, style, scale, locale, window type, visual coverage, semantic coverage, action coverage and capture consistency. Associate each supported combination with a reproducible fixture and named unsupported features.

Inventory actual submitted nodes in default and constrained styles. Rank gaps by affected controls and user workflows, not just node counts. Distinguish exact lowering into existing primitives, a needed negotiated extension, and content that cannot be represented under the strict profile.

Select a second owned application with a different UI structure from the corpus: for example, a small editor with a document list, editable form and dialog. Give it application-owned state and ordinary toolkit callbacks. It must consume the producer SDK without importing `fixtures.gtk`, locating controls by test labels, or depending on the original corpus object.

Completion: the supported target is specific enough to reproduce, and tests can identify whether a failed workflow is an expected coverage limit or a regression. Retain an intentionally unsupported-content case in every coverage run.

### C2 Expand visual lowering with declared semantics

Implement the highest-value exact lowerings first. For every added node family, specify coordinates, clipping, color/compositing, resource ownership and allocation/work limits before enabling it.

Do not assume matching names imply matching rendering. The current exporter accepts only the zero/one endpoints of GTK opacity nodes because partial source opacity has a compositing-contract mismatch with the protocol's linear profile. Likewise, a rounded clip around arbitrary content is more than a rounded rectangle. Gradients, borders, masks, shadows and effects require source-versus-replay evidence for their actual semantics.

When existing primitives cannot express an effect faithfully, choose an explicit negotiated profile extension or keep the gap visible. An extension includes schema/preflight/reducer changes, producer lowering, applicable renderers, independent Python decoding, resource limits and rejection tests for old clients. Add only the bounded vocabulary needed by the selected application.

Use same-submitted-tree captures and native comparisons across scales, transforms, overlap, clipping and text. Retain negative controls that deliberately omit or shift content so tolerance cannot hide a missing control. Photos, video, texture-only content and raster/color glyphs stay unsupported unless a separately authorized source-assets profile is designed; they cannot be silently converted into strict-profile resources.

Completion: the selected controls are complete in the declared matrix, every remaining gap is explicit, and old clients reject unsupported required profiles rather than presenting an incomplete view as faithful.

### C3 Make the cooperating adapter reusable

Extract the contracts currently coupled to `NativeDispatcher.corpus` and the fixture capture objects into application-facing interfaces: source identity, surface registration, coherent capture, semantic registration, guarded action dispatch, invalidation and shutdown.

Provide small toolkit helpers for common controls. They should publish stable roles, typed values/states, text and collection models, relations, available actions and visual bindings. Registration and trusted native object identity determine dispatch; labels and render-tree positions do not. Unknown and unavailable state must remain distinguishable from false and empty values.

Carry source incarnation, object lifetime, focus/selection and model clocks through command checks. Exercise widget replacement, list rematerialization, hidden dialogs, password/redacted fields, repeated open/close and source restart. Coherent capture must still use the node submitted for the local frame, with semantics read at the declared source boundary.

Make export diagnostics and local screenshot/file artifacts optional integration facilities. Their removal from a production path must preserve the verification and ownership boundaries rather than bypassing source validation. Measure any replacement in-memory IPC with P1 before retiring the fixture path.

Completion: both the original corpus and the second application use the same adapter APIs, both accept generic client commands, and adapter code contains no application business rules. Documentation gives a minimal host integration plus the exact conditions under which it may claim coherent capture and checked source dispatch.

### C4 Extend session support one backend at a time

Choose one additional pinned compositor/session environment only after a bounded feasibility investigation establishes its topology, vector-content and permissioned-input hooks. Record how it associates application surfaces, popups, shell content and decorations using trusted identifiers. A list of window titles is insufficient.

Implement the adapter behind the existing session/source interfaces. Keep logical canvas, source surface transforms, granted input regions and physical scale distinct. Reuse the common command ledger and cleanup ownership for focus, modality, local takeover, disconnect, revocation and backend failure.

Test nested/transient windows, clipping, stacking changes, unequal scales, occlusion, stale routes and input held during loss of authority. No broadened whole-desktop permission may be inferred from an app-only grant. Keep tests on an owned nested or isolated session.

Completion: one named additional backend has source-versus-remote action equivalence and declared visual coverage. If it exposes only raster buffers or lacks a trustworthy association/input boundary, publish that feasibility result and retain the narrower scope; do not create a generic desktop-support claim.

### C5 Keep observed accessibility integration separate

Expand AT-SPI roles, text, values or actions only as explicit bounded features with privacy and availability rules. Preserve disposable workers, call deadlines, reconciliation after missed events, and fresh identities after source replacement.

A useful first integration is a read-only semantic view of authorized observed applications. It must display unavailable/stale state and retain `OBSERVED` consistency. Adding actions is a separate contract: determine whether dispatch is checked at the source or explicitly best effort, bind it to trusted application/object identity, and test races before advertising control. Do not derive an injection destination from geometry or a matching label.

Completion: observed clients can distinguish stale, partial and unavailable information; password/private data is excluded; source stalls do not block other clients; any enabled action has a tested, accurately advertised dispatch guarantee. This work does not automatically qualify the visual mirror for the same applications.

## Browser and library maturity

### Starting point

[client-core](../client-core/README.md), [client-wasm](../client-wasm/README.md) and [client-browser](../client-browser/README.md) already provide reusable components. The native desktop still uses `transport::NetworkClient`, while the browser uses the new synchronous session actor. The browser advertises a smaller capability set and its TypeScript model exposes only the fields needed by the mirror demo.

The package is private and experimental. Existing CI covers the Rust workspace and retained native evidence; it does not yet build the browser distribution or execute the WASM/browser suites.

### L1 Add reproducible browser and WASM CI

Add a portable job with pinned Rust/WASM target, a `wasm-bindgen` CLI matching the crate exactly, locked npm installs, a named Node version and owned browser executables. Build the actual module from source before running the Node and browser conformance tests. A host-target Rust build of the binding crate is not a WASM execution test.

Run TypeScript checking, pixel/resource/lifecycle tests and a production Vite build. Test the built output as well as development serving. Add a separate integration job on the pinned GTK/Xvfb environment for host/client edits, observer restrictions, reconnect, hidden tabs, HTTPS, exact Origin validation and cleanup. Do not assume the default CI distribution supplies the required GTK version.

Extend the browser runner to Chromium, Firefox and WebKit where the declared profile is supported. Record actual tested versions. Playwright WebKit alone does not establish Safari/mobile compatibility; include representative device/browser checks before making those claims.

Completion: a clean checkout builds and tests without `/tmp` tools, a warm developer cache or generated files from a prior run. CI retains bounded, sanitized failure artifacts and existing native/replay checks remain enabled.

### L2 Resolve dependencies and consolidate session behavior

First make a behavior matrix for the native and portable actors: profiles/permissions, view selection, readiness, publication assembly, resource paging/eviction, retained resume/reset, command families/results, lease transitions, queued writes, cancellation and disposal. Port missing behavior behind explicit capabilities before replacing native code.

Resolve the dependency direction before wiring `NetworkClient` to the shared actor. Today `rui-client-core` imports the codec from `rui-transport`; making transport depend on core would create a cycle. Extract generated session schema, codec/preflight and portable error/redaction behavior into a proposed `rui-wire` crate, preserving compatibility reexports where useful.

The intended dependency direction below uses arrows from consumer to dependency:

```text
rui-wire -> rui-protocol-core
rui-client-core -> rui-wire, rui-protocol-core
rui-transport -> rui-client-core, rui-wire
rui-client-desktop -> rui-transport, rui-client-core
rui-client-wasm -> rui-client-core
TypeScript browser client -> WASM bindings and browser platform APIs
```

Let the shared actor own deterministic protocol state, negotiated capabilities, readiness, resource/publication transitions, leases and exact pending commands. Native async I/O and browser WebSockets own connection setup, authentication transport, deadlines and bounded send admission. Platform code owns presentation and native/DOM event delivery.

Migrate behavior in slices with shared transcript/fault tests. Preserve cancellation-safe native sending: an await cancelled after sink admission cannot silently drop an acknowledgement or enqueue an admitted frame twice. Preserve browser generation fencing and explicit send-buffer limits. Define owned queued-byte lifetime, send admission and disconnect outcomes in the common API before deleting the old queue.

Completion: the same protocol traces yield the same committed state, events and command outcomes in native and WASM execution; all previously supported native command families still work. Delete duplicate actor behavior only after its native integration tests pass through the shared implementation.

### L3 Define the public integration contract

Separate session options, credentials/transport, state subscriptions, renderer/presentation, and optional interaction adapters. A consumer should be able to replace the renderer or transport and choose a semantic-only mode without constructing a canvas. Keep framework bindings outside the core package.

Specify connection states, terminal versus recoverable errors, reconnect policy, cancellation and disposal. Reject concurrent connection attempts consistently. Define listener lifetime and event ordering, including a callback that disconnects or disposes the client. Cover hidden tabs, browser suspension and page restoration from the back/forward cache. A reconnect never silently reacquires authority; ambiguous commands remain explicitly resolvable.

Expose complete typed state for each supported mode, not unchecked casts that omit text availability, relations, collection metadata or capability information. Preserve `bigint` identities/clocks and scalar-indexed text boundaries. Verify the Rust serialization/TypeScript contract with representative fixtures, including maximum-width integers, null/unknown fields and every enabled record variant.

Define ownership of state copies, resource bytes and renderer caches. Use P4's generation-bound changes to reduce copies if measurements justify them. Avoid exposing mutable views of live WASM memory across calls that may grow memory or change state. Provide explicit full-refresh and resource-miss behavior for custom renderers.

Completion: a small consumer outside the repository can connect, observe, request/release control, issue a guarded command, reconnect and dispose through documented APIs. It uses no fixture internals, ambient globals beyond its declared browser runtime, or direct session-actor field access.

### L4 Add browser capabilities in explicit increments

Do not advertise a profile merely because its Rust helper is shared. Pair each capability with the browser adapter and host/renderer evidence it needs.

| Increment | Required work and acceptance |
| --- | --- |
| Semantic controls | Expose typed advertised actions and parameters, including values and collection actions, with current target/text/model/focus guards. Unsupported action schemas remain disabled. |
| Direct text interaction | Add bounded edit queues, deletion/paste/selection and DOM composition handling. Distinguish local drafts from source preedit. Source composition requires the negotiated protocol, exact editing context and cleanup on blur, removal, revocation, reconnect and hidden tabs. Retain explicit draft submission as a supported mode. |
| Cursor and pointer input | Render negotiated source cursor state; map pointer/wheel/touch through the published viewport and route. Preserve required reliable samples, capture ownership and neutralization on cancellation. Never infer a source target from DOM labels. |
| Physical keyboard input | Define which browser events can be mapped faithfully to the source key model. Browser-reserved shortcuts and unavailable physical information remain explicit limitations. Do not send both a physical key edit and its generated text commit. |
| Source and session controls | Add explicit authorized resize/focus/window operations only for negotiated backends. Local browser resize and responsive layout remain local. Test modality, stale routes and app-only scope. |
| Additional font modes | Retain source outline glyphs as the working baseline. Fingerprint-font support needs verified exact font identity and source glyph positioning; local text substitution or reshaping cannot satisfy it. |

Keep the advertised subset small enough to test across supported browsers. Raw input and source IME parity are separate milestones, not prerequisites for publishing a useful semantic-command library.

Completion: each enabled capability has a capability matrix entry, browser/native event evidence, negative authority/lifecycle tests and accurate consumer documentation. Unsupported capabilities fail before dispatch.

### L5 Produce a distributable package

Define supported browser/Node tooling versions, ESM entry points, TypeScript declarations, asynchronous WASM initialization and how bundlers locate the WASM asset. Document asset URLs, MIME/CSP requirements and initialization failures using a clean consumer fixture. Keep DOM construction out of module import where non-browser tooling needs to load declarations or inspect exports.

Build a package tarball containing only runtime JS, declarations, WASM and required metadata. Install that tarball into a separate minimal Vite consumer and exercise production serving from a non-root path. Verify both the default transport and a replacement transport/renderer. Generated files may stay ignored in the source tree, but release artifacts must contain everything the documented installation needs.

Document compatibility among wire profiles, crate versions, JavaScript API and generated binding version. Add API/asset compatibility checks, changelogs and a defined pre-1.0 breaking-change policy. Review package licenses and required notices before choosing distribution metadata. Publishing to a registry is a separate release action after the package is reviewable.

Provide host integration examples for trusted direct WSS and a certificate-verifying proxy. Keep credential provisioning caller-owned and out of bundles, URLs and automatic browser storage. Preserve exact Origin allowlists and document the explicit HTTP/WS development mode used on a trusted tailnet.

Run a two-machine acceptance check with the host listening on machine A and a browser-only client on machine B over the tailnet. Cover hostname/IP addressing, exact Origin configuration, WASM loading, trusted HTTPS and bidirectional edits. Keep this evidence distinct from the existing tests that bind `0.0.0.0` and connect over loopback.

Completion: installation from the built tarball works on a clean consumer, the compatibility and support matrix is explicit, the two-machine workflow is demonstrated, and CI exercises the distributed artifacts rather than only repository-relative imports.

## Semantic presentation

### Starting point

The [independent Python client](../conformance/semantic_client.py) already discovers semantic records and performs button activation, Unicode editing and stable-item selection without using scene coordinates. The native transport implements `SemanticReady`. The portable session currently requires mirror/glyph capabilities and presentation before control, so the existing browser client cannot simply replace its canvas with DOM controls.

This work deliberately advances the semantic-presentation portion of RFC section 14. It retains one authoritative application state and one remote control lease. Independent application views and concurrent writers remain separate scope.

### S1 Add semantic-only session mode

Introduce explicit mirror and semantic-only session configuration in the shared actor and public bindings. Follow the existing host's profile and OpenView contract; semantic-only is an override within the current negotiated profile scheme, not a guessed removal of every base profile name. It must not require glyph resources or a visual renderer.

Add SemanticReady for the current applied semantic revision, clear it on reset/reconnect/resource or scope changes as required by the existing contract, and gate semantic commands on it. Reject Presented and raw input/composition/resize in this mode. Keep structural validation of transmitted topology even though DOM layout does not consume its coordinates.

Audit shared command builders for visual-readiness assumptions. Preserve common authority and typed preconditions while allowing a validated semantic target to be acted on without a visual Binding or a complete scene surface. Do not bypass host validation or widen a lease to make the new client work.

Completion: the Rust/WASM semantic client completes the same three-effect scenario as the independent Python client, receives no Scene/Binding/cursor resources in its semantic-only projection, emits no Presented, and cannot use readiness to authorize another command family.

### S2 Build a generic read-only semantic view

Extend the TypeScript contract to include ordered children, relations, typed values/states, text models, collection materialization, semantic grouping, reading order and completeness. Start a separate demo consumer, proposed as `demos/semantic`, using the reusable library.

Render native DOM elements according to a small published mapping of roles and capabilities. Use semantic ownership/grouping and reading order for layout; never use host scene coordinates, GTK class names, fixture labels or hardcoded IDs to infer a control. Missing roles or unsupported actions render as bounded informational content with their availability made clear.

Use stable protocol identities as DOM keys. Preserve local keyboard focus and drafts across harmless updates, but retire them on removal or scope replacement. Represent partial collections as partial, including known totals; offscreen or unmaterialized items are not an empty list. Resolve label/description relationships within the authorized projection.

Render remote strings as text or values, never executable HTML, scripts, CSS or automatic network URLs. Add explicit limits for materialized DOM nodes and update work. Large lists may use bounded virtualization that retains the meaning of partial materialization and keyboard navigation.

Completion: two differently structured source applications produce useful read-only DOM views through the same mapping. Renaming labels and changing IDs does not change control selection logic. Unknown states, unavailable text, redaction and incomplete collections remain distinguishable.

### S3 Connect semantic controls to guarded commands

Map only advertised actions and supported schemas to commands. A checkbox/value control uses the declared value action; collection selection uses the declared stable-item/collection action and model version. Do not invent a new wire action because a DOM widget normally has one.

Separate local DOM focus/navigation from source focus. Observers can navigate, select local text and resize their browser without mutating the host. Controllers request source focus only for an explicit operation that requires it, then wait for confirmed source context before capturing an edit.

Begin with draft text plus explicit apply, retaining the exact target, text/model/focus clocks and lease context. Support Unicode scalar conversion, paste, deletion and locally composed committed strings. Display pending, rejected, conflicting and ambiguous results. Do not silently overwrite a newer host edit or retry a stale draft with fresh preconditions. Direct per-keystroke editing can reuse L4's queue once its causal ordering is proven.

Keep the last authoritative value visible while a local operation is pending; any optimistic draft must be visibly local and reconcilable. On revocation or changed read scope, discard unauthorized draft/cached content and disable mutation immediately. Hidden-tab control release and reconnect behavior match the mirror client's authority policy.

Completion: generic controls perform the button/text/collection scenario with exactly the intended source effects, observers cannot mutate, and stale target, text, selection, focus, model and lease contexts reject safely. Host-side edits update both semantic and mirror clients consistently.

### S4 Add responsive layout and accessible interaction

Define local layout rules for groups, labels, forms, collections and dialogs. Use semantic relationships and reading order to change from a wide layout to a narrow stacked layout. Browser resize must not send source resize, alter application scroll or modify host layout.

Use keyboard-operable native controls with associated names/descriptions and clear focus, pending and error states. Keep remote source focus and local navigation focus distinguishable. Implement modal presentation from published relationships without trapping local observers in a source mutation flow. Test screen-reader output and keyboard navigation in addition to inspecting the DOM tree.

Exercise at least 320 px, 800 px and 1440 px viewports, long labels, Unicode/bidirectional text, zoom, collection updates and dialogs. Touch gestures initially operate the local semantic controls. Do not translate an arbitrary gesture into raw host input or claim a separate per-client application view.

Completion: the same application state is usable at all declared layouts; source records and layout remain unchanged by local reflow; names, reading order, keyboard traversal and error announcements pass the declared accessibility checks. Actual mobile-browser support requires device evidence in addition to viewport emulation.

### S5 Prove the presentation boundary

Run a real host with a semantic controller and visual observer, then swap control. Provision distinct approved client instance IDs and credentials; the current manual demo's single approved identity cannot be reused for simultaneous clients. Record authorized exact protocol evidence, command effects and final source/client state, comparing each client's authorized projection rather than assuming private fields are identical across clients. Tests must discover targets through semantic structure/actions; fixture-specific labels may appear only in assertions about expected source content.

Cover reconnect/reset, a stale draft, removal and rematerialization, source restart, permission reduction, partial collections, modal changes, delayed command results and loss of control mid-edit. Demonstrate operation with visual records absent and with arbitrary changes to source geometry. Keep the independent Python client as an external schema-level oracle; replacing it with the shared Rust implementation would remove that independence.

Completion: a generic alternative presentation works for two owned applications, its declared capabilities and gaps are documented, and exact traces prove that its commands depend on semantics rather than scene coordinates. Publish this as semantic presentation support for that scope, without extending the claim to arbitrary applications or concurrent collaboration.

## Validation commands and completion records

Run commands from the repository root. These are existing entry points at the baseline; new tests introduced by a work item must be added to its CI job and documentation. Native commands require the pinned environment and an owned Xvfb executable. Use a new output directory for capture measurements.

Core and independent replay:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p rui-producer-sdk --features transport,gtk-fixture-hooks --all-targets --locked
python3 conformance/check.py
python3 -m conformance.typed_interop
python3 -m conformance.full_interop
python3 -m unittest adapters.sdk.test_producer adapters.gtk4.test_capture adapters.gtk4.test_native
python3 -m unittest conformance.test_semantic_client conformance.test_full_reference
```

WASM, browser and production assets, after installing the matching CLI documented in [client-wasm](../client-wasm/README.md):

```sh
python3 client-wasm/build.py
npm --prefix client-wasm ci
npm --prefix client-wasm exec -- playwright install chromium
npm --prefix client-wasm test
npm --prefix client-browser ci
npm --prefix client-browser test
npm --prefix demos/web ci
npm --prefix demos/web run build
```

Actual visual interaction and capture:

```sh
cargo build --release -p rui-client-desktop --features gtk-demo --bins --locked
python3 demos/check.py --scenario local --xvfb /path/to/Xvfb
RUI_BROWSER_HEADED=1 RUI_BROWSER_HTTPS=1 RUI_BROWSER_BUILD=1 RUI_WASM_CHROMIUM=/path/to/chromium python3 demos/web/check.py --xvfb /path/to/Xvfb
python3 fixtures/gtk/measure.py --xvfb /path/to/Xvfb --output /tmp/rui-coverage-NEW
```

Existing large-workload measurements:

```sh
cargo run --release -p rui-transport --example measure_integrated --locked
cargo test -p rui-client-desktop --test performance_source --release --locked -- --ignored
```

Use the additional native source, composition and session commands in [GTK coverage](gtk-coverage.md), [desktop controls](desktop-controls.md) and [session integration](../spec/session-shell-profile.md) when those paths change. A package unit test or retained trace replay does not replace a fresh native check for a changed source/input boundary.

For each completed step, record the commits, capability/profile changes, tested environment, commands, evidence paths, measured limits and remaining exclusions. A workstream is complete only for the applications, browsers and backends its evidence actually covers. Keep the current interactive demo usable throughout the sequence so each improvement can be tried directly.
