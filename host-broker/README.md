# Controlled headless host

`ControlledHost` runs a cooperating application with an increment button and a
private text field. The host owns all business logic and source clocks. Clients
receive exact Protobuf envelopes and send exact Protobuf `SemanticCommand`
payloads. No client code executes application logic.

Run the two-client fixture:

```sh
cargo run -p rui-host-broker --example controlled
cargo test -p rui-host-broker
```

The example negotiates a controller and an observer, feeds their snapshots and
updates into `ViewReceiver`, retries an increment, and prints public counts.
Its presentation callback is simulated. This crate supplies no TLS server,
credential verifier, GUI renderer, arbitrary application capture, or OS input
injector.

The trusted transport/auth adapter supplies `VerifiedAuthorization` and binds
subsequent calls to the `AuthenticatedClient` returned by negotiation. Never
accept either value as a client-declared credential. `submit` always joins the
command ledger, current lease/permissions, relevant semantic versions, and
source validation; callers cannot access mutable source or core components.
`ReplaceText` uses Unicode scalar ranges and requires current text and focus
versions. Actual application commits produce `SourceEvidence`; a result names a
view revision only after that view's transaction was successfully published.

Private content requires the explicit trusted permission
`fixture.read-secret.v1` in `Permissions::actions`. Control alone does not grant
content access. Other views have a redacted semantic node and text model, no
private edit action, and a fixed public rectangle. The private string and its
length never influence that scene. Publications and retained recovery history
are independently projected per client. Diagnostics and errors contain no
private command arguments or source text. This proves the controlled fixture's
privacy behavior, not general redaction for arbitrary applications. Previously
disclosed copies held by an authorized recipient cannot be recalled.

`checkpoint` explicitly installs a coherent snapshot, advances the view
revision, and resets history and readiness. `snapshot` reads the current
published capture without resetting anything; use it with a fresh replica or
for inspection. Feeding it to an existing replica at a transaction's revision
conflicts with exact-byte replay validation. A disconnected view that missed
source changes requires a new checkpoint rather than a delta with skipped
object versions. Reconnection never restores control or presentation readiness.

Snapshot and update outputs are full, bound `Envelope` bytes. This fixture
requires each envelope to fit the negotiated envelope ceiling; a chunked
publication producer is deferred. The core receiver supports chunk assembly.
Returned `SharedPayload` handles remain charged until their last clone drops.
The source limits private text to a configured maximum of 64 KiB; readable views
reserve its worst-case decoded size. Client metadata, ledger entries, outcomes,
history, and outbound queues use the core's bounded retention. A failed or slow
view closes explicitly while other authorized views continue.

For a nonvisual consumer, explicitly require and advertise `semantics.only.v1`. The host constructs a distinct projection without Scene/Binding records or resource references, preserving semantic/text/source focus and surface topology with Unsupported visual coverage. `acknowledge_semantic_ready` follows `acknowledge_applied`, permits only semantic-family dispatch, and never fabricates a presentation acknowledgement. The source broker now admits all families through the shared controlled ledger; this baseline fixture rejects families that its cooperating application does not implement.

`ControlledHost::new_corpus(config, secret)` opts into the expanded cooperating source. It adds a public Unicode field with backward scalar selection and source focus, a desired Boolean checkbox, three stable materialized list items with total count ten, stable-ID selection/reordering, a clipped scroll viewport, a second toplevel, and fresh popup/modal/tooltip incarnations. List-item activation requires the current version of its published collection owner. The no-argument reorder button needs no undiscoverable collection precondition. Modal state disables main-window actions and moves source focus to its dismiss target, then restores focus. Incarnations are bounded and deleted IDs are never reused.

The corpus currently declares its main surface **Partial**, with `source-glyph-provider-pending`; valid Unicode text, including supplementary characters, remains available semantically. The vector canvas currently publishes its bounded scene region; raw event tracing, composition, authorized resize, native glyph resources and retained network recovery remain subsequent integration work. This slice does not claim complete visual mirroring. `view_description` reports the actual source canvas and selected scope instead of advertising baseline dimensions or complete coverage.

`source_capture(client, view)` reconstructs records directly from the authoritative cooperating source, independently of encoded publications or a client replica. It requires the current published capture context and exposes borrowed `records()`, validated `state()` and `source_commit()` under an opaque view-memory reservation. Direct source conversion checks graph, identities, geometry and model limits; it establishes neither source provenance nor resource availability. The capture's Debug omits content. Its current graph has no receiver retirement history. Consumers own and must budget any copies they create.

Per-view projection metadata retains bounded ID/version/digest stamps. Each transaction gives a changed record exactly the next version; an explicit checkpoint can catch up skipped source clocks. Text-content, collection and focus clocks change only with their corresponding source state. Snapshot/checkpoint and publication capture reserve conservative scratch before producing records, fingerprints or cloned mutations. Retained capture outputs, encoded snapshots/history/queues and scratch share the same negotiated view ceiling. Small budgets may permit a live view yet reject an additional capture/read workspace; failed reads leave revision, acknowledgements and dependent history intact.
