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

Snapshot and update outputs are full, bound `Envelope` bytes. `snapshot` keeps
its single-envelope ceiling. The checkpoint producer and queued transactions
may exceed that ceiling within the negotiated whole-snapshot/transaction and
view-memory limits; transport must deliver them through bounded
begin/chunk/commit messages, with every actual message within its envelope
ceiling. The direct receiver convenience method still has a single-envelope
limit. Returned `SharedPayload` handles and their resource attachments remain
charged until their last clone drops.
The source limits private text to a configured maximum of 64 KiB; readable views
reserve its worst-case decoded size. Client metadata, ledger entries, outcomes,
history, and outbound queues use the core's bounded retention. A failed or slow
view closes explicitly while other authorized views continue.

For a nonvisual consumer, explicitly require and advertise `semantics.only.v1`. The host constructs a distinct projection without Scene/Binding records or resource references, preserving semantic/text/source focus and surface topology with Unsupported visual coverage. `acknowledge_semantic_ready` follows `acknowledge_applied`, permits only semantic-family dispatch, and never fabricates a presentation acknowledgement. The source broker now admits all families through the shared controlled ledger; this baseline fixture rejects families that its cooperating application does not implement.

`ControlledHost::new_corpus(config, secret)` opts into the expanded cooperating source. It adds a public Unicode field with backward scalar selection and source focus, a desired Boolean checkbox, three stable materialized list items with total count ten, stable-ID selection/reordering, a clipped scroll viewport, a second toplevel, and fresh popup/modal/tooltip incarnations. List-item activation requires the current version of its published collection owner. The no-argument reorder button needs no undiscoverable collection precondition. Modal state disables main-window actions and moves source focus to its dismiss target, then restores focus. Incarnations are bounded and deleted IDs are never reused.

The corpus currently declares its main surface **Partial**, with `source-glyph-provider-pending`; valid Unicode text, including supplementary characters, remains available semantically. The vector canvas currently publishes its bounded scene region; raw event tracing, composition, authorized resize, resource-bearing network delivery and retained network recovery remain subsequent integration work. This slice does not claim complete visual mirroring. `view_description` reports the actual source canvas and selected scope instead of advertising baseline dimensions or complete coverage.

`source_capture(client, view)` reconstructs records directly from the authoritative cooperating source, independently of encoded publications or a client replica. It requires the current published capture context and exposes borrowed `records()`, validated `state()` and `source_commit()` under an opaque view-memory reservation. Direct source conversion checks graph, identities, geometry and model limits; it establishes neither source provenance nor resource availability. The capture's Debug omits content. Its current graph has no receiver retirement history. Consumers own and must budget any copies they create.

Per-view projection metadata retains bounded ID/version/digest stamps. Each transaction gives a changed record exactly the next version; an explicit checkpoint can catch up skipped source clocks. Text-content, collection and focus clocks change only with their corresponding source state. Snapshot/checkpoint and publication capture reserve conservative scratch before producing records, fingerprints or cloned mutations. Retained capture outputs, encoded snapshots/history/queues and scratch share the same negotiated view ceiling. Small budgets may permit a live view yet reject an additional capture/read workspace; failed reads leave revision, acknowledgements and dependent history intact.

`new_corpus_with_fonts(config, secret, Arc<dyn SourceFontProvider>)` adds an
explicit trusted source-native shaping capability for public captions. The
provided `NativeSourceFontProvider::new(python_path, helper_path)` uses the
repository's pinned Pango/Cairo/Liberation Sans helper over bounded local IPC;
paths come from the host, and no shell or client-supplied program executes.
Private fields never enter this capability, including when a view can read the
field. The helper emits verified normalized vector outlines with source glyph
positions. The producer preserves source painting order, caps each caption at
128 glyphs, reserves 16 fixed caption sets, and keeps dormant IDs owned by an
empty main-window group. Unchanged captions retain runs/resources. Current
source resources have independent 512-entry/8MiB ceilings, with at most old and
new source closures coexisting during serial refresh; native IPC separately
bounds request/output bytes, decoded lists and its process deadline.

A client selecting `glyph.outline-single.v1` receives the exact authorized
outline closure and Complete coverage when all source captions are supported.
Clients without that opt-in receive Partial vector coverage. Valid text edits
remain applied when source shaping fails, including supplementary glyphs
missing from the pinned font; affected visual coverage is explicitly Partial.
There is no client shaping, font substitution, raster atlas or font-file wire.

`SourceCapture::resources()` borrows exact `(ResourceOffer, body)` pairs from
the same source capture. `resource(client, view, digest)` rechecks the current
registered authenticated projection and returns an `AuthorizedResource` whose
borrowed bytes remain charged through its last handle. Digests alone grant no
permission. Resource closures are shared by current projection, snapshot,
history, outbound and capture handles. Old still-live closure owners remain
available even when newer owners drop first. The weak authority registry is
bounded to 512 digests and 4096 owner references, pruned before publication;
its metadata fits the conservative decoded projection reservation. Source
bodies and registry content never appear in automatic Debug/errors. Network
offer/chunk/ack retention is a separate integration gate, not claimed here.

The default tests are pure Rust. Run the explicit native font gate on the pinned
source environment (GTK4.22.4/Pango1.58.2/Cairo1.18.4, Python GI/Cairo and the
bundled verified font), without an availability-based skip:

```sh
cargo test -p rui-host-broker --test source_fonts -- --ignored
```
