# Bounded cooperating-source input

`protocol-core::input` provides interpretation and state machines, not authority or operating-system injection.
A host adapter must first authenticate the client, validate the current lease and view, and admit the original command bytes through the shared epoch command ledger.
It must evaluate routing, focus, modality and hit testing at the same source boundary as dispatch.

Each `InputRouter` belongs to one lease, seat, device and surface.
Fresh events require the source-issued 16-byte routing token and the presented revision.
Client coordinates are inverted through the presentation transform; letterboxing is excluded.
The source explicitly supplies its logical bounds and granted EIS region.
Physical display pixels are never used to infer EIS geometry.

Events remain ordered, including every drag sample.
Physical keyboard codes use Linux evdev values and host repeat; client-generated repeated presses fail.
Client-text mode excludes this profile's physical keyboard events.
Pointer motion with a held button and motion for an existing touch can continue outside the surface; unrelated new presses and touches cannot use another stream's capture.

A complete batch is validated before dispatch.
Before external callbacks, the router retains conservative cleanup ownership of all possibly pressed inputs.
An error or panic blocks fresh dispatch until the trusted adapter drains and executes releases/cancels.
Cleanup releases bypass stale routing and focus.
The host must finish cleanup before allowing another lease or changing router scope.

`CompositionMachine` binds transient preedit to a target, focus generation, text version and client-text owner.
Context changes cancel composition.
Scalar ranges count Unicode scalar values.
Commit consumes a composition identity once and returns a private `TextEdit`; the authoritative text engine must atomically check and apply that edit before the host reports source-backed success.
Grapheme navigation and host IME behavior remain source responsibilities.

These low-level objects have local event, held-input, composition-count and text byte ceilings.
Their allocations are not part of the replica's shared `MemoryBudget`.
The host must bound their number, queued events and retained edit outputs; constructing arbitrary numbers of routers does not establish a combined session memory bound.

`protocol-core::transcript` separately records authorized post-handshake traffic under the exact principal/session/epoch/view binding.
Metadata-only recording is the ordinary policy; exact bytes require explicit trusted diagnostic policy.
Credential handshakes must never enter the recorder.
Retained entries, spare container capacity and diagnostic bodies are charged to its shared memory budget.
Revocation purges retained entries and permanently closes that recorder.

## Cooperating GTK client-text composition

The cooperating GTK producer advertises `text.client.v1` to a full visual controller explicitly granted `TextComposition`.
Admission requires its actual current Presented view, the shared command sequence/byte ledger and the current lease.
The focused editable target, focus generation, text content version, complete text-record version, scalar selection, native surface, source incarnation and lease owner are checked again on the serialized GTK thread.

Begin, UpdatePreedit and Cancel acknowledge actual private source transient state as `Dispatched`.
They produce no document edit, source-commit token, publication or observer preedit.
Commit consumes the original Begin scalar range once, applies the native GtkEditable value and caret, then waits for the existing fresh whole-view capture barrier.
`Applied.source_commit` names the newly captured editing Surface's token in `source_capture_tokens`; the whole-view request cause, source generation and capture nonce must already have passed the source audit.
Exact retries return the common ledger's retained result.

`text.clock.gtk-editable4224.v1` is an optional GTK 4.22.4 clock contract.
This producer offers it when its native capture declares that exact toolkit version; the client must explicitly support it.
It is absent from core session defaults and the required profile set.
Source replacement cannot change the pinned toolkit version beneath an existing broker epoch that offered this contract.

GTK's observed internal clear/insert steps can advance clocks several times within one authorized value replacement.
The measured profile preserves every native observation.
For the source's serialized GtkEntry value/caret operation, the content-clock delta equals `before_nonempty + after_nonempty`, where each boolean contributes zero or one.
The full text-record delta equals that content delta plus one when the final scalar caret is greater than zero.
Thus nonempty replacement or insertion with a positive final caret advances content by two and the full record by three; a prefix deletion with caret zero advances both by two; deletion to empty advances both by one; insertion into empty advances content by one and the record by two; an empty-to-empty operation advances neither.
A same-content nonempty replacement still records the native clear and reinsert transitions.

A receiver uses these precise deltas only after negotiating this profile, with the exact expected final text and scalar caret, unchanged editing identity/focus, and an authenticated matching Applied token in the coherent installed editing Surface.
Extra transitions invalidate the pending editing context.
Receivers without this optional contract retain their ordinary conservative clock checks.
Independent focus, content and selection away/back observations remain intact.

Composition identities are retained up to 128 per source incarnation; preedit, commit and resulting document text each have a 16 KiB UTF-8 ceiling.
The private composition allocation is reserved from the source owner's shared memory budget.
Focus/text/selection changes, removal and failed native observations clear transient state before a fresh context.
Revocation/disconnect retire the native atomic lifecycle fence immediately; source-thread polling clears preedit, and every late commit checks that fence and execution-time expiry before effects.
Private transient diagnostics belong only to explicitly enabled controlled fixture hooks.
