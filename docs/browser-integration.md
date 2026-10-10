# Browser integration contract

Native transport and the WASM session now share the Rust authenticated `Replica`
state machine, exact command ledger and resource cursor. Browser transports and
renderers remain replaceable adapters; they never decode/reduce wire state themselves.

`BrowserClient.create` initializes WASM asynchronously and validates the WSS URL.
Module import constructs no DOM. Exported `initialize(optionalWasmUrl)` performs
bounded asynchronous module preflight before binding initialization; 404/corrupt/foreign
ABI failures can be retried, and a loaded module cannot switch URLs. Construction requires a browser document, a
caller-owned credential provider, explicit source selection and a presentation
adapter. `canvas` supplies the default `CanvasRenderer`; `renderer` and `transport`
can be replaced independently. `mode: semanticOnly` requires neither canvas nor renderer;
its consumer explicitly calls `semanticReady` after accepting a current revision.

The default transport verifies certificates through the browser and requires the
`rui.v1` subprotocol. Credentials are copied into the handshake, cleared after use
and never placed in URLs, bundles or automatic storage. A replacement transport
must authenticate ordered binary frames, admit/copy outgoing bytes synchronously,
report bounded buffered bytes, and deliver callbacks asynchronously after returning.
It owns any admitted copy and clears it after transmission/disposal. The library
clears its outgoing copy immediately. The Rust actor retains exact ambiguous command
bytes until a definitive result or scope retirement.

Connection states are `idle`, `connecting`, `connected`, `disconnected`, `disposed`.
Concurrent connection attempts reject before another credential request. There is
no automatic reconnect or authority reacquisition. Call `connect` explicitly after
a recoverable transport failure; a disposed client cannot reconnect. Invalid wire,
profile, authority or resource input closes the current connection. Protocol errors
require correcting the peer/configuration before retrying. `retryPending` resends
only original bytes and is an explicit ambiguity recovery operation.

`subscribe` returns an idempotent unsubscribe callback. Events are delivered in
insertion order to a snapshot of subscribed listeners; removals take effect before
delivery and additions begin at the next event. Disconnect/dispose inside a callback
stops the old generation's delivery and processing. Exceptions propagate to the
connection failure handler; listeners should contain their own application errors.
`dispose` removes listeners and browser hooks, retires receiver/commands and releases
renderer caches. Closing/disconnecting never claims presentation or control cleanup
before the host observes the connection closure.

A hidden document releases control and cancels pending presentation. Returning to
visibility re-renders the current state; it does not silently request control. For
page restoration, the embedding application should disconnect on `pagehide` and
explicitly reconnect after `pageshow`; automatic back/forward-cache reconnection is
not promised. Browser suspension still requires the host's lease expiry/neutralization.

`state` returns an owned structured copy of the last committed graph. All record
variants and enabled fields are declared in `model.ts`, including unknown tri-states,
nullable text/value/relations, collection totals/materialization, surface topology,
source capture metadata and cursor appearance. Identities, revisions, source clocks,
scalar boundaries, signed stacking and integer values use `bigint`. Remote strings
are data; consumers must use text/value APIs rather than HTML or URL injection.
No mutable WASM memory view survives a call. Resource access makes bounded owned
copies; caller/renderer JavaScript allocations have their own limits. Renderer
inputs are an owned generation-bound export and must be treated as read-only.

Custom renderers implement `render(state, viewport, resource)` synchronously and
`dispose`, optionally `reset`. The callback and public `resourceBytes(digest)` return
owned verified bytes; missing/unauthorized/disconnected resource IDs reject. Consumers
own returned copies. `resourcesReady` indicates whether the committed graph's closure
is verified. `refresh` requests a full current-frame repaint without a source command.
A custom renderer failure closes the connection, so errors need explicit consumer
recovery rather than a false presentation acknowledgement.

`resourceCacheLost` is a trusted local policy signal. It rejects while a publication
is staged, preserves committed records/exact commands, clears visual readiness and
renderer caches, announces loss and requests the live closure in bounded pages.
Rendering pauses while required bodies are missing. Verified refill resends Applied;
only a following actual frame opportunity restores mirror presentation readiness.
A resource-free graph is a no-op and does not disturb semantic readiness. Retained
resume with a lost closure likewise waits for verified bodies rather than acknowledging
an unusable revision. `evictUnpinnedResource` removes at most one retired body and
returns its digest; current/candidate dependencies remain pinned. These APIs do not
grant resource authorization or change the source. Reconnect still requires explicit
authority acquisition.

Complete Python/native/WASM serialization and three-engine lifecycle tests cover all
record variants and maximum-width clocks/typed values. Shared core tests exercise a
512-body closure and repeated cache loss with 1024-byte messages, unique bounded
request pages, unchanged revision and zero ownership after disposal. Fresh live
editor evidence uses the public API to pause an active mirror lease, restore exact
owned bytes, and wait for actual presentation. The L5 clean consumer installs the runtime tarball outside the checkout and tests
production assets and both adapter seams; see browser-distribution.md. External
physical-device/two-machine acceptance remains unverified.

Control request IDs include a fresh random actor namespace, current authenticated
scope and monotonic counter. Recreating a client for the same approved identity cannot
reuse a retained host request outcome. Reconnect preserves its namespace/counter and
exact command bytes; it never restores control. Low-level Rust/WASM Session consumers
must set a nonzero platform-generated request namespace once before connecting.
BrowserClient supplies one with crypto.getRandomValues.

An approved client instance ID identifies one command producer for a host epoch.
Use `disconnect`/`connect` on the same actor to preserve its sequence and exact pending
bytes. Final `dispose` discards that ledger; a new mutating actor needs a separately
approved client instance ID or a new host epoch. Reusing a disposed producer identity
can observe/request control, but its reset command sequence is rejected by the host.
The library does not infer a sequence or recreate an old ambiguous operation.
