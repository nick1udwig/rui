# Native and portable session behavior

`rui-wire` owns the bounded codec, schema, errors and zeroizing wire ownership.
`rui-client-core::replica::Replica` now owns the authenticated synchronous state
machine used by both `NetworkClient` and the WASM-facing `Session`. Transport,
deadlines, GUI events and actual presentation opportunities remain adapter duties.

| Behavior | Shared actor | Adapter policy |
| --- | --- | --- |
| Negotiation | version, bindings, limits, profiles and permissions validated once | native caller hello; portable explicit mirror/semantic-only subsets |
| Publications | exact digest/chunk assembly, Rust reducer, retirement and closure | ordered authenticated packet delivery |
| Readiness | Applied distinct from current Presented or SemanticReady | actual native/browser frame; explicit semantic acceptance |
| Resources | bounded cursor, verified offers/bodies, pinned eviction and cache loss | native async or portable synchronous charged replies |
| Resume/reset | retained committed receiver, exact commands, scoped rebind and readiness clearing | explicit reconnection/authentication; no authority restoration |
| Leases | charged held grant, matching revocation and readiness/authority clearing | native request IDs and held-input cleanup; portable stale-response fencing |
| Commands | shared builders, sequence space and exact retained family/payload | native negotiated semantic/raw/composition/resize; portable tested semantic subset |
| Malformed input | poisons the connection and clears staging/control/readiness | close transport; committed graph/exact pending commands survive explicit reconnect |
| Disposal | clears receiver, commands, resources and capability reservations | release socket/writer, DOM/GUI listeners and derived render caches |

Applied, Rejected and Failed retire a command. Unknown, OutcomeUnknown,
OutcomeExpired, Accepted and future statuses retain its exact bytes. Dispatched
retains semantic edits, resize and IME Commit. For raw input and IME Begin,
UpdatePreedit or Cancel, Dispatched confirms the native submission/transient stage
and retires it. It never claims an IME document edit or presentation.

The native sink retains charged encoded bytes and its submitted marker until a
successful flush, including cancellation after sink admission. Portable admission
removes queued bytes only when the platform takes/copies them; browser socket
buffer limits and generation fencing remain platform duties. Reservations precede
reply construction. Escaped view metadata and held leases retain their charges.

Failed reauthentication, allocation and cancelled I/O preserve the native retained
receiver and exact pending commands. No retry recreates an ambiguous command with
new authority. Same-epoch reconnect queries/retries original bytes; a changed epoch
retires them. Queue, message, resource and memory ceilings are unchanged.

Regression evidence includes dual native/portable packet-prefix and reply equality,
foreign binding poisoning, all command-family receipt policies, native TLS lost-result
recovery/allocation failure and cancellation tests, three-engine WASM tests, and
fresh owned GTK IME/two-client editor checks. See `fixtures/browser/shared-actor`
and `fixtures/gtk/shared-actor`. Historical M0–M5 acceptance traces remain separate.
