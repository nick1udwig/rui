# RUI/0.1: Remote UI State Replication

**Status:** historical initial design, not an external standard.
**Date:** 2026-10-01.

The original bundle contained this design and a machine-readable conformance-test specification, without a runtime or executable test suite.
The repository now includes implementations and extensions beyond that original scope.
See [current status and direction](docs/status.md), [implementation acceptance](conformance/STATUS.md), and the [profile index](docs/README.md#protocol-design-and-profiles) for the implemented system.

## 1. Decision and scope

Build a transport-independent replication protocol with three related models: surface topology, retained visual state, and semantic UI state.
The host owns application state, focus, shared layout, and command execution.
Clients retain authorized projections and either replay the visual state (initial implementation) or construct a different presentation from semantic state (later implementation).

Wayland is a local integration layer, not the exported UI model.
Its normal rendering boundary carries application-rendered buffers [S1].
AT-SPI is an optional semantic adapter, not the source of faithful visuals [S2].
The ext-foreign-toplevel-list protocol is deliberately minimal and does not provide a complete desktop composition model [S3].

The first complete vertical slice is a controlled application/session with cooperating producers.
The next slice is a pinned, instrumented GTK4 application.
Arbitrary unmodified desktop applications, browser canvases, games, compositor decorations and shell UI are NOT implicitly supported.
Coverage is a published property, not an assumption.

The initial client displays the host's logical canvas without relayout.
Client-side scaling, panning and letterboxing do not resize the authoritative canvas.
Resizing the host is a separate authorized request.
Several clients can observe; one remote client can hold a control lease.
Local host activity revokes that lease under the initial policy.

### 1.1 Pixel policy

RUI/0.1 requires `content.no-raster.v1`.
No framebuffer, screenshot tile, UI raster fallback, glyph atlas, image resource, or video frame may be transmitted in that profile.
It allows geometry, colors, text, shaped glyph positions, exact references to preinstalled fonts, and vector-outline resources.
No font files are included in this bundle.

An optional future `content.source-assets.v1` profile may allow original raster assets such as photographs.
It must be explicitly selected; it is not a backdoor for rasterizing UI.
A raster-only source without an approved alternative produces `UNSUPPORTED_CONTENT`.
Turning each screenshot pixel into a rectangle is also a raster-derived fallback and is not compliant.

The strict profile cannot faithfully communicate previously unknown photographs using no raster representation.
This is a scope boundary rather than an encoding problem.

### 1.2 Fidelity claims

`mirror2d.core.v1` promises the same supported scene, host layout and application state, subject to a declared renderer profile.
It does not promise bit-identical results across arbitrary fonts, GPUs or rasterizers.
Reference conformance uses a pinned renderer/environment.
Unsupported effects must not silently disappear.
A surface is either complete for its negotiated profile, explicitly partial, or unsupported.
Only complete surfaces count towards a faithful-mirroring acceptance gate.

## 2. Foundations and alternatives

* **GTK4/GSK:** use its retained render nodes as a visual producer and, initially, as a client rendering backend.
  Translate into an independently versioned wire schema.
  `GskRenderNode.serialize()` has only same-GTK-version guarantees and is intended for testing/debugging, not a stable storage format [S4].
  Cairo and texture nodes need explicit treatment; GSK does not imply an all-vector source [S5].
* **AccessKit:** borrow its stable semantic node IDs, tree updates and action-request model.
  It is a semantic/accessibility foundation, not a scene renderer or a complete network session protocol [S6].
* **Shopify Remote DOM:** a stronger starting point when all participating UIs can be authored as controlled web components.
  It replicates UI mutations and forwards interactions, but does not extract arbitrary desktop applications; network authentication, recovery and authority rules remain additional work [S7].
* **Qt Remote Objects:** a useful authoritative Source/Replica foundation for owned Qt application models.
  It does not automatically export arbitrary application windows and all their visual state [S8].
* **Broadway:** useful implementation precedent, not the selected protocol.
  GTK's documentation describes it as experimental and not the most actively developed backend; its documented GTK4 design also used images for text [S9, S10].
* **Waypipe/gfxstream:** Waypipe transports buffer changes; gfxstream forwards graphics API calls.
  Neither yields semantic UI state by itself.
  GPU command streams are not proof that raster data is absent [S11, S12].

## 3. Architecture

```text
Authoritative application / toolkit / controlled desktop shell
        |                         |
 semantic capture             visual capture before rasterization
        |                         |
        +---- source adapter -----+
                      |
        broker: authorization, IDs, projections, revisions,
                resources, snapshots, command dispatcher
                      |
           authenticated bidirectional transport
                      |
          client replica: staged and committed state
                      |
             +--------+----------------+
             |                         |
       mirror renderer          later semantic translator
             |                         |
             +------ typed commands ---+
```

A cooperating producer can publish a visual/semantic capture from one source commit.
A best-effort AT-SPI observer cannot manufacture that guarantee from independent calls.
Broker atomicity means the replica publishes one valid broker transaction at a time; it does not imply that independently sampled applications changed atomically.

Source adapters publish `capture_consistency = COHERENT | OBSERVED`.
`COHERENT` requires producer-supplied capture boundaries.
`OBSERVED` carries independent semantic and visual capture tokens and must not claim that they describe the same source frame.
An action source may separately advertise `dispatch_consistency = CHECKED_AT_SOURCE | BEST_EFFORT`.

## 4. Identity and versions

| Field | Meaning |
|---|---|
| `session_id: bytes16` | Logical hosting session. |
| `epoch: bytes16` | Authority incarnation. Changes on restart or loss of deduplication/identity state. |
| `client_instance_id: bytes16` | Client command producer, bound to authenticated principal. |
| `view_id: bytes16` | One authorized projection, including selected surfaces, profile and policy. |
| `view_revision: uint64` | Ordered publication revision for that projection. Initial snapshot is revision 1. |
| `object_id: uint64` | Host-assigned nonzero ID, never reused in an epoch. |
| `object_version: uint64` | Version of that record, initially 1, increased on replacement. |
| `command_seq: uint64` | Contiguous client command sequence within `(epoch, principal, client_instance_id)`. |
| `source_capture_token: bytes` | Opaque source-issued provenance, not a global clock. |

Object references also carry their type.
IDs are not pointers, D-Bus addresses, titles, row positions or renderer allocations.
A deleted object's ID cannot be reused.
Restarted adapters retire their old IDs and invalidate outstanding source-bound commands.
A new view is required when authorization or required profiles change.
Semantic IDs remain stable across scene rebuilds whenever the underlying semantic object survives.

The broker may use an internal authority sequence; clients need not receive it.
Exposing only per-view revisions avoids depending on unrelated, unauthorized applications' update counters.

## 5. Replicated records

### 5.1 Surface

```text
Surface {
  id, version, role: TOPLEVEL | POPUP | TOOLTIP | SHELL,
  parent_surface?, desktop_rect, content_size,
  transform, clip?, opacity, stacking_order,
  mapped, focus_state, modal_for?,
  scene_root, semantic_root?, input_region,
  routing_token,
  visual_coverage: COMPLETE | PARTIAL | UNSUPPORTED,
  semantic_coverage: RICH | PARTIAL | OPAQUE,
  capture_consistency, source_capture_tokens,
  unsupported_features[]
}
```

The session includes a logical canvas and cursor state, including shape or vector resource, hotspot and position.
Popups and tooltips are first-class surfaces.
A full-desktop claim also requires shell UI, composition transforms, clipping, decorations and stacking; a list of application titles does not satisfy it.

### 5.2 Semantic node

```text
SemanticNode {
  id, version, role,
  label?, description?, typed_value?, states,
  ordered_children[], relations[],
  actions[], text_model?, collection_model?,
  visual_bindings[], semantic_group?, reading_order?,
  sensitivity, completeness
}
```

Roles and action names are protocol vocabulary, not toolkit class names or translated labels.
States are typed and may be unknown; absent must not be interpreted as false unless a field explicitly defines that default.
Relations include label-for, labelled-by, controls, describes and group membership.
A parent is derived from ordered child lists, not independently writable.
Semantic relations may form a general graph; the ownership tree may not contain cycles.

A visual binding can reference a surface, zero or more visual instance IDs, and an optional explicit interaction region.
The binding is many-to-many.
A semantic button may correspond to several visual nodes; decorative visuals may have no semantic counterpart.
Wire render-node identity must not determine semantic identity.

An opaque custom canvas still has a semantic boundary and declared available interactions; it is not falsely described as translatable.

### 5.3 Scene instance

```text
SceneNode {
  id, version, ordered_children[],
  affine_transform, clip?, opacity,
  primitive: Group | SolidRect | RoundedRect | Border |
             LinearGradient | RadialGradient | FillPath |
             StrokePath | GlyphRun,
  primitive_parameters,
  resource_refs[]
}
```

These are retained scene instances, not per-frame GPU calls.
Path and glyph-outline data can be immutable content-addressed resources.
Multiple scene instances can share one resource but retain distinct IDs.
Scene ownership is an acyclic tree in the initial profile; resource sharing does not require shared scene parents.

Coordinates use host logical units with top-left origin, x right and y down.
Rectangles are half-open.
Wire numeric geometry uses finite IEEE-754 binary64 values, normalizes negative zero to positive zero and rejects NaN/infinity.
The affine matrix explicitly maps local to parent coordinates.
Bounds and path lengths are validated before allocation or drawing.

The rendering sub-spec fixes premultiplied linear-sRGB source-over composition, gradient stop rules, path fill rules and stroke cap/join conventions.
Unsupported color spaces/effects require negotiation, not guessed conversion.
A producer must lower its supported source representation to these semantics or report a gap.

### 5.4 Text

Keep semantic text separate from visual glyph runs.
A `TextModel` includes UTF-8 content or explicit unavailable/redacted status, text version, selection, and edit capabilities.
All protocol text ranges count Unicode scalar values; they are not bytes or UTF-16 code units.
Adapters convert at their boundary.
User-facing cursor navigation still obeys grapheme boundaries in the authoritative text engine.

A `GlyphRun` contains an exact font fingerprint or a vector-outline resource, size/variation parameters, glyph IDs, host-shaped positions and optional text-cluster mapping.
The mirror client does not reflow or reshape host text.
The initial profile uses preinstalled, fingerprint-verified fonts; a mismatch is `MISSING_RESOURCE` or a negotiated outline alternative, never silent substitution.
Actual Unicode text remains available separately when authorized, so a later translator need not infer text from glyph geometry.

Collections record stable item identities, optional total count and explicit materialization coverage.
Missing/offscreen children are not an empty collection.
Initial fixtures may fully materialize small collections.
Paged queries are a later capability with revision-bound cursors, not a promise that accessibility can expose an application's entire database.

## 6. Transport and negotiation

Initial binding: authenticated TLS WebSocket, binary Protobuf envelopes, one envelope per WebSocket message.
The protocol remains transport-independent.
Disable WebSocket compression initially.
Chunk resources and large publications so control messages can be interleaved.
A single TCP connection has a latency tradeoff; do not claim independent delivery streams.
QUIC/WebTransport can be a later binding after measuring this limitation.

Authentication precedes application data.
Use existing TLS/authentication mechanisms, host approval and principal-bound credentials.
A client-declared ID is not authentication.
The initial deployment can pair a client with a pinned host identity and explicit local approval.
Do not expose raw AT-SPI/D-Bus, Wayland object handles or EIS file descriptors over the network.

```text
ClientHello {
  supported_protocol_versions[],
  supported_profiles[], required_profiles[],
  renderer_capabilities, input_capabilities,
  receiver_limits, client_instance_id,
  resume_request?
}
ServerHello {
  selected_version, selected_profiles[],
  session_id, epoch, granted_permissions,
  negotiated_limits, resume_disposition
}
OpenView { selection, mirror_canvas_policy }
ViewOpened { view_id, canvas, coverage, granted_input_modes }
```

Version selection occurs within the authenticated connection.
Unknown optional fields may be ignored.
Unknown required profiles, primitives, mutation variants or behavior-changing extensions must be rejected.
Extensions have namespaced, versioned identifiers; the host may lower an unsupported feature only when equivalence is guaranteed by its registered lowering rule.

Default receiver ceilings, all negotiable downward: envelope 1 MiB; chunk data 64 KiB; transaction 16 MiB; snapshot 64 MiB; individual resource 16 MiB; total live plus staged view memory 128 MiB; queued outbound data per client 8 MiB; 100,000 records; ownership depth 128.
Limits include decoded sizes and allocation budgets, not just compressed wire bytes.
These are proposed starting defaults, not measured optimums.

## 7. Message families

| Family | Messages |
|---|---|
| Session | `ClientHello`, `ServerHello`, `OpenView`, `ViewOpened`, `CloseView`, `Ping`, `Pong`, `Error` |
| Resources | `ResourceOffer`, `NeedResources`, `ResourceChunk`, `ResourceComplete`, `ResourceAvailable`, `ResourceEvicted` |
| Publication | `SnapshotBegin`, `SnapshotChunk`, `SnapshotCommit`, `TransactionBegin`, `TransactionChunk`, `TransactionCommit`, `AbortPublication` |
| Recovery | `AppliedAck`, `PresentedAck`, `ResumeRequest`, `ResumeAccepted`, `ResetRequired` |
| Control | `AcquireControl`, `ControlGranted`, `RenewControl`, `ReleaseControl`, `ControlRevoked` |
| Commands | `SemanticAction`, `RawInputBatch`, `TextComposition`, `ResizeRequest`, `CommandResult`, `GetCommandResult` |

Every post-handshake envelope is bound to the negotiated session/epoch and, where applicable, view.
Message type and payload length are bounded before deeper parsing.
Large transaction and snapshot payloads are byte-chunked independently of record boundaries.

## 8. Resources

`ResourceOffer` specifies kind, version, total bytes and a SHA-256 digest of a domain-separated header and exact resource bytes.
The header includes fixed-width kind/version/length fields, so concatenation is unambiguous.
Hash exact received bytes; do not deserialize and reserialize a Protobuf message to check integrity.
Protobuf deterministic serialization is not a cross-language canonicalization guarantee [S13].

Resource bytes are immutable.
A client acknowledges availability only after full receipt, bounds validation, digest verification and successful safe decoding.
Resource lookup requires current view authorization.
A known hash alone is not authority to retrieve data from another view or principal.
Default caches are session/principal-scoped and memory-only for sensitive content.

Resources used by live, staged or in-flight scenes stay pinned.
A client may evict only unpinned resources and informs the host.
A missing resource holds a publication in staging while `NeedResources` is serviced; it does not cause a half-rendered commit.
Timeout/budget failure causes reset or explicit resource failure.
No resource permits arbitrary external URL fetching, arbitrary shaders or executable code in the initial profile.

## 9. Snapshot and transaction semantics

### 9.1 Snapshot

The host creates an immutable checkpoint at a specific view revision and retains subsequent history while transmitting it.
Snapshot chunks carry an ID and contiguous chunk index.
Commit declares total length/counts and the digest of exact concatenated payload bytes.
The client verifies the full record graph and required resources before swapping it into committed state.

Updates that occur during snapshot transfer are not included opportunistically.
They follow as transactions based on the checkpoint.
If bounded history is insufficient, the host explicitly resets to a new checkpoint.
It never sends a silently inconsistent initial tree.
Input is disabled until a usable snapshot is presented and control is granted.

### 9.2 Transactions

```text
Transaction {
  base_revision, new_revision = base_revision + 1,
  mutations: PutSurface | PutSemanticNode | PutSceneNode |
             PutTextModel | PutBinding | PutSessionState |
             DeleteRecord,
  required_resources[], source_capture_metadata,
  caused_by_commands[]   // only when supported by real source evidence
}
```

`Put*` is full-record replacement; omitted optional fields are cleared.
A new record has version 1.
Replacement increments the previous record version by one.
Reparenting changes affected child lists in one transaction.
Deletion is explicit and non-cascading: all dangling references, bindings and focus references must be repaired in the same transaction.
IDs remain retired after deletion.

The client stages all mutations, then validates references, record types, ownership cycles, depth, sizes, versions, policy and resource closure.
It publishes all records and the revision in one atomic swap.
Failure leaves prior committed state untouched.
Wire order within the transaction does not require parents to precede children.

A transaction whose base is not the current revision is rejected without mutation and triggers resynchronization.
A duplicate whose revision/digest is still known is acknowledged but never reapplied.
An old transaction outside that remembered range is rejected as stale, never interpreted as a new mutation.

### 9.3 Acknowledgements and recovery

`AppliedAck(revision)` means replica state and resource closure were accepted.
`PresentedAck(revision, surface_presentation_tokens)` means the renderer presented that revision; it does not prove a human saw it.
A client may apply several revisions and render only the newest complete one.
It must still apply intervening dependent state, or receive an explicit snapshot replacement.

Resume requires the same epoch, retained view, authorization, selected profiles and usable acknowledged state.
Otherwise the host sends `ResetRequired`.
A restarted renderer may retain state but must reacquire missing resources before presenting.
A restarted authority changes epoch and invalidates stale commands.
The client resets transient input state on every new connection and must reacquire control.

Each client has bounded queues and acknowledgements.
Slow clients do not block other clients or the host UI thread.
The broker can pause production for that view, send a fresh checkpoint, or close it with `SLOW_CONSUMER`.
It cannot drop arbitrary dependent mutations.
After repeated failed resets, close the view rather than grow memory without limit.

## 10. Commands, input and authority

### 10.1 Semantic command

```text
SemanticAction {
  epoch, view_id, client_instance_id, command_seq,
  control_lease_id, control_generation,
  target_id, action_id, action_schema_version,
  observed_view_revision,
  preconditions: { target_version?, text_version?, model_version? },
  typed_arguments
}
```

Initial standard actions are `Activate`, `SetValue`, `ReplaceText`, `SetSelection`, `ScrollTo`, and `Focus`.
Applications may expose namespaced typed actions with explicit schemas.
Prefer desired-state operations such as `SetChecked(true)` over a retry-sensitive `Toggle`.
No arbitrary method invocation is allowed.

The host validates current authorization, target membership, capability, lease, schema and relevant preconditions at dispatch.
Unrelated animation does not invalidate a command; a changed text/model version can.
A deleted/disabled target, unsupported action or stale input context produces an explicit rejection.
`observed_view_revision` records causality context, not a blanket compare-and-swap of the entire desktop.

### 10.2 Results and deduplication

Statuses are `ACCEPTED`, `DISPATCHED`, `APPLIED`, `REJECTED`, `FAILED`, `OUTCOME_UNKNOWN`, and `OUTCOME_EXPIRED`.
`ACCEPTED` is admission, not execution.
`DISPATCHED` means passed to the adapter/application, not durable success.
`APPLIED` requires application-level evidence and may reference a resulting view revision; a successful operation need not change visible state.
An AT-SPI or raw-input adapter must not invent this evidence by correlating timestamps.

Within a live epoch, `(principal, client_instance_id, command_seq)` is an at-most-once dispatch key.
Sequences are contiguous.
A rejected well-formed command consumes its sequence as well.
The host retains a per-client high-water mark even after detailed outcomes expire.
Retransmission uses the same key and exact command bytes; it returns a stored result or `OUTCOME_EXPIRED`, never dispatches again.
Reusing a key with different bytes is a protocol violation; gaps are rejected.

`GetCommandResult` is read-only and does not allocate a new action sequence.
A connection loss between an external side effect and its recorded result can leave `OUTCOME_UNKNOWN`.
Across authority restarts, there is no generic exactly-once guarantee for external applications.
Old-epoch commands are rejected.
Clients must not silently repeat an ambiguous click or create a fresh command ID for an automatic retry.
Stronger guarantees require an idempotency/transaction mechanism inside the authoritative application.

### 10.3 Raw input

`RawInputBatch` is one ordered command containing pointer, key, wheel or touch events.
It includes target surface, last presented revision, host-issued routing token, device/seat identity and monotonic event timestamps.
Pointer coordinates are host surface-local logical coordinates.
The client reverses its own zoom/letterbox transform; the host revalidates surface topology, hit testing, clipping, focus, modality and capture.

A routing token changes when changes affect that input route; it need not change for unrelated animation.
A stale token rejects fresh presses, not cleanup releases or cancels needed to unwind existing input.
Raw key identity is physical and namespace-qualified; the host adapter maps it to its input system and authoritative layout.
Key repeat ownership is negotiated; the initial raw mode uses host repeat only.

Initial authoritative input is reliable and ordered.
Do not drop/coalesce pointer paths blindly: dragging, drawing and hover transitions may depend on them.
A future datagram pointer-hint channel would not be an authoritative action channel.

For local Linux integration, EIS/libei via the Remote Desktop portal is the input adapter; the portal recommends this path [S14].
EIS coordinates refer to granted regions, which may not equal actual desktop pixels, so explicit mapping is necessary [S15].
EIS injection is not intrinsically atomic with application hit-testing.
A scoped controller that cannot enforce authorized input routing must disable that mode or use an isolated, fully authorized desktop session.
Direct cooperating-app dispatch is preferred for strict per-app control.

### 10.4 Text input / IME

Negotiate `text_owner = HOST_IME | CLIENT_TEXT` per focused editing context.
In host-IME mode forward physical keys and let the host compose.
In client-text mode use `CompositionBegin`, `UpdatePreedit`, `CommitText` and `CancelComposition` with a text-session ID, focus generation, base text version and scalar-indexed ranges.
Preedit is transient, not committed document text.

Never send both composing physical keys and the corresponding text commit.
Non-text shortcuts use explicitly separate key handling.
A focus/context change invalidates a composition and triggers cancellation.
Adapters lacking high-level text support advertise that limitation; libei alone is not a semantic editing API.

### 10.5 Control lease

A view starts read-only.
The host grants one remote writer for the shared seat, with generation, expiry and allowed operations.
Proposed initial expiry is five seconds, renewed once per second.
Host-local activity takes priority and revokes remote control.
Disconnect detection, lease expiry or revocation cancels composition/touch and releases all keys/buttons held by that controller.
Cleanup events bypass ordinary stale-routing rejection.

Control-management requests have request IDs.
Repeated acquisition by the same requester returns its existing live lease, not a second lease.
Renewals require the current generation; repeated release is idempotent.

Several observers may have independent zoom, panning and local navigation state.
None may mutate authoritative selection, focus, text, application scroll or window geometry without permission.
Independent simultaneous editing seats/CRDT collaboration are not implied by multi-client observation and are out of scope.

## 11. Security and privacy

Authorize before producing the projection.
Apply redaction to semantic data, visual data, resources, errors, traces and snapshots consistently.
A redacted semantic field must not remain readable in a visual glyph resource or cached old snapshot.
Password entry may travel encrypted from an authorized controller to the host, but must not be echoed to observers or retained in ordinary logs.

No unrequested clipboard, file transfer, audio, clipboard read, shell commands or cross-application D-Bus access.
Use bounded parsers and sandbox graphics/resource decoding where practical.
Treat the host as untrusted input to a client renderer as well as clients as untrusted command sources.
The source adapter cannot grant itself broader capture permission than its host session.

Revocation stops future disclosure; it cannot make a previously authorized malicious client forget information.
Purge cooperative caches and close/reset projections on scope change.
Resource digests are integrity identifiers, not authorization tokens.

## 12. Implementation plan and gates

### M0 — Feasibility and vocabulary

Choose a pinned GTK build and representative application corpus.
Inspect actual pre-raster render-node types for labels, entry fields, menus, popovers, lists and a custom canvas.
Measure unsupported Cairo/texture/effect regions.
Prove a same-frame visual-capture hook and a semantic source with stable IDs.
Public WidgetPaintable/snapshot APIs are useful for experiments [S16], but faithful capture of the actual submitted frame may need a small pinned GTK submission hook.
Do not assume a normal Wayland observer can extract it.

**Gate:** a written coverage report distinguishes exact supported capture, observed semantics and unsupported content.
A strict vector fixture contains no raster fallback.
This gate runs before investing in a broad desktop integration.

### M1 — Pure protocol core

Implement model validation, full-record reducer, transactional staging, resource manager, bounded framing, command ledger and transcript recorder.
Suggested core language: Rust, with generated Protobuf types and a small independent reference model in a second language.
Keep toolkit types out of the public schema.

**Gate:** all core/model/command conformance cases pass without a GUI or live network.
Deliver schema, exact transcript vectors, reducer API and property-based tests.
The files in this bundle specify that work; they do not implement it.

### M2 — Controlled non-translating vertical slice

Build a host fixture application that produces semantic and scene state, renders its own authoritative scene locally, and serves two desktop clients.
Use GTK4/GSK as the first client backend or another pinned 2D backend capable of the negotiated primitive set.
The client receives no business logic.
Include controls, text editing, a stable-ID list, scrolling, popup/modal surfaces and a vector canvas.

**Gate:** one client controls and another observes; both converge with the host after actions, reconnects and resource-cache loss.
Resize only the observer window and verify that host layout does not change.
Compare host-local output with the wire-replayed output under the same reference rendering profile.

### M3 — Real GTK producer

Capture the render tree actually submitted per surface before rasterization.
Export supported node types to the wire model.
Obtain semantic records/actions from in-process instrumentation for reliable mapping; optionally add AT-SPI as a separately classified best-effort adapter.
Associate popups and surfaces using explicit producer identities, not titles or geometry guesses.

Keep local rendering and remote export fed by the same captured source tree.
A toolkit hook/fork may be needed; this is an explicit engineering cost.
GSK serialization is useful for local diagnostics, not the network contract.
Renderer-node grouping does not automatically provide widget semantics.

**Gate:** an ordinary GTK app in the supported corpus passes visual, interaction, lifecycle and no-raster checks.
Unsupported controls/themes are named and surfaced.
AT-SPI crawl/update races are repaired or reported as `OBSERVED`, not hidden.

### M4 — Desktop/session integration

Add compositor-specific/session-shell topology and permissioned libei input.
Model shell surfaces, menus, cursor and window composition.
Prefer an isolated controlled session before supporting an arbitrary live desktop.
Isolate blocking accessibility calls from the host's publication loop and use reconciliation after event gaps or application restarts.

**Gate:** multiwindow geometry, modality, scaling, focus, local takeover and input-region mapping pass on each explicitly supported compositor/backend.
A whole-desktop claim is allowed only for the exported session and content coverage actually tested.

### M5 — Hardening and translation seam

Add fuzzing, network fault injection, long-run resource churn and independent-decoder interoperability.
Implement a nonvisual semantic-only test client that activates a button, edits text and selects a stable collection item without reading scene coordinates.
This is not a mobile UX implementation; it verifies the protocol boundary required for one.

**Gate:** fixed-state snapshots and incremental replay produce identical reference state.
Slow readers stay bounded.
A newly built independent client can interoperate from the schema and golden transcripts.
Detailed resource, CPU, latency and bandwidth measurements accompany results; no bandwidth advantage is assumed in advance.

Suggested repository boundaries:

```text
spec/                  normative schema and profile documents
protocol-core/         validation, reducer, resources, command ledger
host-broker/           auth, views, history, leases and dispatch
adapters/sdk/          coherent producer contract
adapters/gtk4/         pinned visual + semantic integration
adapters/atspi/        optional observed semantic bridge
adapters/linux-input/  portal/EIS integration
client-desktop/        retained mirror renderer and input mapping
client-semantic-test/  nonvisual actions; no mobile layout yet
fixtures/              controlled app and pinned assets/environment
conformance/           transcript runner, properties, fault injection
```

## 13. Test methodology

The companion `conformance.yaml` is an initial specification, not an executable runner.
Cases define fixtures, operations and observable expected outcomes.
A future runner must expose committed/staged state, dispatch ledger, resource ownership and decoded pre-encryption protocol records.

Use four independent oracles: a small reference state reducer; authoritative application state/action traces; local rendering comparisons; and decoded wire/policy inspection.
Local screenshots are test artifacts only, never a remote rendering transport.
Searching encrypted network packets for PNG magic bytes is not a no-raster test.

Property testing generates valid and invalid graph transactions, duplicate/replayed commands, resource lifetimes and disconnect points.
Network tests vary latency, bandwidth and connection failures; transport-level packet loss must not be mistaken for permission to drop ordered state messages.
Test application-level duplicates and malformed transcripts separately.

Reference visual tests pin fonts, renderer, scale, color behavior and animation time.
Their exact output is the baseline.
Cross-backend/GPU tests use explicit per-fixture tolerances with independent geometry/glyph-position assertions so antialias tolerance cannot hide missing controls.
Functional input tests compare effects, not just screenshots.

Performance targets are provisional acceptance budgets to measure on named hardware.
Suggested starting workload: 10,000 records, one edit changing at most 20 records, 60 Hz presentation opportunity, two healthy clients plus one throttled observer.
Measure publication/encode/apply/render p50/p95/p99, action-to-present latency, resource-cache hit rate and transmitted bytes.
A one-node edit must not resend the entire tree or cached resources; the implementation's own queue/memory ceilings must hold.
No universal sub-frame or network bandwidth guarantee is made.

## 14. Deliberately deferred

Mobile layout and gesture translation; independent per-client application views; concurrent writers; collaboration/CRDT semantics; general GPU command streaming; audio/video; clipboard/file transfer; arbitrary image fallback; executable remote components; browser engine capture; desktop protocol standardization across compositors.
Each can be added through negotiated capabilities without changing the core separation between semantic identity, scene presentation and authoritative commands.

## References

These sources support the technology assessment, not the newly proposed protocol requirements.

[S1] Wayland FAQ: `https://wayland.freedesktop.org/faq.html`  
[S2] AT-SPI API: `https://gnome.pages.gitlab.gnome.org/at-spi2-core/libatspi/`  
[S3] ext-foreign-toplevel-list protocol source: `https://raw.githubusercontent.com/wayland-mirror/wayland-protocols/main/staging/ext-foreign-toplevel-list/ext-foreign-toplevel-list-v1.xml`  
[S4] GSK serialization contract: `https://docs.gtk.org/gsk4/method.RenderNode.serialize.html`  
[S5] Cairo render-node constructor: `https://docs.gtk.org/gsk4/ctor.CairoNode.new.html`  
[S6] AccessKit architecture: `https://github.com/AccessKit/accesskit/blob/main/ARCHITECTURE.md`  
[S7] Remote DOM: `https://github.com/Shopify/remote-dom`  
[S8] Qt Remote Objects: `https://doc.qt.io/qt-6/qtremoteobjects-index.html`  
[S9] Broadway documentation: `https://docs.gtk.org/gtk4/broadway.html`  
[S10] Broadway GTK4 design, 2019: `https://blogs.gnome.org/alexl/2019/03/29/broadway-adventures-in-gtk4/`  
[S11] Waypipe developer notes: `https://mstoeckl.com/notes/code/waypipe_to_rust.html`  
[S12] gfxstream: `https://github.com/google/gfxstream`  
[S13] Protobuf serialization is not canonical: `https://protobuf.dev/programming-guides/serialization-not-canonical/`  
[S14] Remote Desktop portal: `https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html`  
[S15] EIS region API: `https://libinput.pages.freedesktop.org/libei/api/structei__region.html`  
[S16] WidgetPaintable: `https://docs.gtk.org/gtk4/class.WidgetPaintable.html`
