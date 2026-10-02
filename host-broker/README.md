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
