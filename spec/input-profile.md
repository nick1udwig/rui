# Bounded cooperating-source input

`protocol-core::input` provides interpretation and state machines, not authority
or operating-system injection. A host adapter must first authenticate the client,
validate the current lease and view, and admit the original command bytes through
the shared epoch command ledger. It must evaluate routing, focus, modality and
hit testing at the same source boundary as dispatch.

Each `InputRouter` belongs to one lease, seat, device and surface. Fresh events
require the source-issued 16-byte routing token and the presented revision. Client
coordinates are inverted through the presentation transform; letterboxing is
excluded. The source explicitly supplies its logical bounds and granted EIS
region. Physical display pixels are never used to infer EIS geometry.

Events remain ordered, including every drag sample. Physical keyboard codes use
Linux evdev values and host repeat; client-generated repeated presses fail.
Client-text mode excludes this profile's physical keyboard events. Pointer motion
with a held button and motion for an existing touch can continue outside the
surface; unrelated new presses and touches cannot use another stream's capture.

A complete batch is validated before dispatch. Before external callbacks, the
router retains conservative cleanup ownership of all possibly pressed inputs.
An error or panic blocks fresh dispatch until the trusted adapter drains and
executes releases/cancels. Cleanup releases bypass stale routing and focus. The
host must finish cleanup before allowing another lease or changing router scope.

`CompositionMachine` binds transient preedit to a target, focus generation, text
version and client-text owner. Context changes cancel composition. Scalar ranges
count Unicode scalar values. Commit consumes a composition identity once and
returns a private `TextEdit`; the authoritative text engine must atomically check
and apply that edit before the host reports source-backed success. Grapheme
navigation and host IME behavior remain source responsibilities.

These low-level objects have local event, held-input, composition-count and text
byte ceilings. Their allocations are not part of the replica's shared
`MemoryBudget`. The host must bound their number, queued events and retained edit
outputs; constructing arbitrary numbers of routers does not establish a combined
session memory bound.

`protocol-core::transcript` separately records authorized post-handshake traffic
under the exact principal/session/epoch/view binding. Metadata-only recording is
the ordinary policy; exact bytes require explicit trusted diagnostic policy.
Credential handshakes must never enter the recorder. Retained entries, spare
container capacity and diagnostic bodies are charged to its shared memory budget.
Revocation purges retained entries and permanently closes that recorder.
