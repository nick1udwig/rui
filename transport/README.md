`rui-transport` binds the controlled headless host to real rustls TLS and tungstenite WebSockets. `ServerHandle::bind` accepts the host identity, bounded `PairingAuthority`, host fixture and cleanup callback. `NetworkClient::connect` requires an exact pinned DER certificate, a matching hostname, a locally approved credential and a registered producer ID. The certificate also passes normal rustls verification. No projection is constructed before authentication and authorization.

`ProducerServerHandle::bind` uses the same authentication, framing, diagnostics
and delivery path with a `ProducerPort`. The trusted adapter owns one source,
broker and common command ledger; compound requests keep negotiation/resume
keepers, readiness, admission and publication/resource selection in that owner.
Issued connections carry an opaque owner/incarnation capability. Reusing a view
ID cannot revive an old connection, and final delivery rechecks current capability
and approval before attempting a write. Source envelopes must match the issued
session/epoch/view before their payload receives a socket binding.

The reference `ControlledPort` preserves the original shared fixture host for
tests. It admits at most 32 normal requests under an 8 MiB owner budget, with
conservative request/reply reservations and separately charged escaping immutable
handles. One precharged persistent lifecycle task owns disconnect obligations
even if a flush is busy or cancelled. It terminates after shutdown/final owner
drop without retaining an owner cycle. A pending disconnect slot cannot be reused
before its exact cleanup completes; cleanup failure permanently blocks new work.
Lease checks add elapsed owner-queue time to the caller's monotonic timestamp.
ACK/authorization/pump operations do not require extra view scratch allocation,
so releasing readiness is still possible at the negotiated view-memory limit.

Adapters must retain those lifecycle, timing and allocation contracts in their
own owner lane. This transport boundary alone does not establish native injection,
toolkit capture or graphical presentation; those adapters have separate gates.

Run `cargo test -p rui-transport --test loopback --test resources --test reconnect` with loopback socket access. The resource tests use a bounded synthetic outline provider and cover cold paged closure delivery, warm updates, live cache loss, premature acknowledgments, invalid digest declarations and local approval revocation. Actual font rendering and GUI presentation are separate explicit native integration gates. Tests connect a controller and an observer through actual TLS, apply chunked binary Protobuf publications through `ViewReceiver`, edit Unicode scalars at the source, retry identical commands, reject observer dispatch, preserve private-field redaction and close a slow observer independently. Retained reconnect tests interrupt a chunked transaction, preserve committed state/cache, replay offline changes, restore lost resources and reject stale leases. The checked-in test certificate/private key are public test fixtures, never deployment identities.

For concrete local pairing, run `cargo run -p rui-transport --example pair_and_serve -- CERTIFICATE.der PRIVATE_KEY.pkcs8.der NEW_CREDENTIAL_FILE`. It displays the public certificate fingerprint, producer ID, principal and exact requested permissions. The operator must type `APPROVE` before the credential is generated. The example creates a new owner-only file and never prints its bearer credential. Approval lives only in this process. A deployed application must supply its own local identity provisioning, secure credential delivery/storage and operator UI. No remote message can approve or widen access. Revocation invalidates the immutable grant even if the same principal is immediately approved again.

`spec/session.proto` is a separate generated wire module. Every WebSocket application message is one binary `Packet`; JSON/text transport and compression are rejected. A bounded HTTP upgrade on both sides selects the allocation ceiling before any credential envelope is read. Frame/message limits and send deadlines are configured in the maintained framing library. The server allows at most `max_connections` tasks. Recursive schema preflight counts merged nested fields, packed values and repeated strings before prost decode, with a conservative decoded allocation ceiling of `4 * envelope_bytes + 4096` per message. Commands reach the host's exact-byte ledger once; current authorization and lease/source preconditions remain mandatory inside it.

Publication begin/chunk/commit frames borrow a lifetime-budgeted `SharedPayload`; reading its revision/count does not decode or copy its graph. Each chunk and encoded packet is bounded by the negotiated envelope and chunk ceilings. Input/control packets interleave between publication chunks. Applied acknowledgement follows validated receiver installation; presentation acknowledgement requires the explicit renderer callback. The trusted cleanup callback executes with the host mutex held, must be bounded/nonblocking, and must perform held-input cleanup. A callback panic leaves a persistent acquisition gate closed.

Upstream tungstenite contains trace calls that can include plaintext frames. This crate therefore enables the `log` facade's `max_level_off` and `release_max_level_off` features. Cargo feature unification disables **all log-facade output in the resulting binary**, including other crates using that facade. No credential or private command is automatically formatted by Packet/ClientHello Debug; transient owned credential copies are zeroized. These measures do not prevent a trusted application from deliberately printing its own private state.

Whole Snapshot/Transaction envelopes are bounded by `snapshot_bytes`/`transaction_bytes` and the shared view memory ceiling. They may exceed `envelope_bytes` only through begin/chunk/commit publication; every actual socket Packet still obeys `envelope_bytes`. Read-only single-message snapshots retain their stricter envelope bound. The optional glyph profile admits a trusted source font provider's verified outline closure. After validating the exact publication and graph, the pinned authenticated producer grants only resources referenced by that graph; extra unreferenced declarations reject the publication. The server independently rechecks current local approval, principal/view and the exact retained publication's closure before selecting offers or bodies. A known digest alone never authorizes a read.

Resource requests page through at most 512 digests. Immutable resource ownership stays charged through publication history, outbound frames, source captures and the sender's retained APPLIED frame. Cold installation preserves the old live state until every dependency is verified; warm updates reuse the authorized verified cache. Full live-cache loss declares its missing closure, clears readiness, and restores APPLIED only after all bodies and ResourceAvailable acknowledgements complete. The sender retains broker-stamped transaction proof so an acknowledgment still succeeds if its bounded recovery history has evicted that in-flight frame; checkpoint, policy/view and budget changes invalidate the proof. Resource waiting has a bounded deadline, and dependent publications wait rather than silently dropping deltas.

`NetworkClient::evict_unpinned_resource()` releases one retired cache entry and sends its notification without clearing live readiness. It reserves queue capacity and shared memory before cache mutation; cancellation retains the encoded notification. Current/pending dependencies remain pinned. The host remembers at most 4096 producer-declared digests per view, with a conservative 512 KiB reservation taken before allocation. These declarations include unsent/offline publications and do not prove that a peer cached a resource. They permit harmless eviction bookkeeping only: body delivery still requires membership in the exact in-flight or last APPLIED closure. Retired notifications prune metadata; live loss starts recovery and blocks readiness. Mixed batches validate every ID and duplicate before mutation, and arbitrary/unknown eviction IDs close the connection. Metadata exhaustion closes the affected publication view explicitly instead of growing the set.

The server reserves `8 * envelope_bytes + 128 KiB` lifetime framing/decode headroom, `3 * envelope_bytes + 64 KiB` concurrent ingress headroom, and 512 KiB bounded delivery metadata against the host view budget before post-authentication sends. Publication and resource bodies retain their own shared reservations. A budget too small for these conservative charges rejects the connection.

The authenticated connection polls a socket reader concurrently with its serialized producer operations in the same task. A native owner waiting for a callback therefore cannot hide WebSocket Close or TLS EOF. The reader retains at most 32 frames under a byte quota of one envelope plus 8 KiB of entry overhead; overflow closes that exact connection instead of waiting behind normal producer work. Queued bodies are zeroized when dropped. Reader termination cancels the pending response wait and invokes the producer's independent disconnect path. It does not turn an already admitted native operation into a claimed success or undo a completed effect. Real TLS regressions in `tests/producer_disconnect.rs` cover close, EOF and ingress saturation while normal producer work remains blocked.

Semantic, raw input, text composition and source resize share the common exact-byte
ledger. The controlled corpus implements permission-filtered raw canvas effects,
transient preedit/scalar commits and bounded source resize; the baseline counter
source rejects unsupported families. OS input and native presentation are separate
adapter/client integrations. Private-field filtering is guaranteed only for this
controlled cooperating fixture, not arbitrary applications.

`ViewOpened.input_devices` is an optional tag-128 extension identifying the
approved view's source seat, device, key namespace, input mode and text ownership.
Absence disables raw device discovery. The receiver allows at most 16 unique
seat/device scopes and 64 bytes per string; it rejects overwritten fields,
duplicates and unsupported mode/ownership/namespace combinations before raw use.
The current modes are `raw.pointer.v1` with `CLIENT_TEXT`, and `raw.host.v1` with
`HOST_PHYSICAL`, using `linux.evdev`. Observer and semantic-only projections
advertise no devices. Every command still needs the current lease, permission and
source route checks. `ClientEvent::ViewOpened` owns an immutable `OpenedView`
reservation against the receiver budget, including when retained outside the
client; callers must account for any explicit clones of protobuf fields.

The runnable pairing example uses `HostConfig::fresh()` before creating its
in-memory dispatch ledger. A broker restart or lost ledger must mint a new epoch;
retained client state then receives a fresh checkpoint. `tests/epoch.rs` preserves
the TLS identity and approval across broker replacement, loses the first action's
reply, and verifies that both ambiguous and never-sent old-epoch commands close
without dispatch or sequence consumption, even after a fresh lease is acquired.
The new epoch accepts sequence one only when encoded under its new authority.

`NetworkClient::into_retained()` drops its socket, staged publication/resource transfers, unsent replies and presentation state. It keeps the committed reducer, retired identities and live resource cache. Call `RetainedClientView::reconnect(...)` with the same credential-bound producer ID and no caller-supplied resume request, then call `open_view()` on the returned client. A failed or cancelled reconnect preserves the retained owner for retry. Only the authenticated `PRODUCER_CONNECTION_BUSY` response retries automatically, within two seconds, while the previous connection finishes cleanup; pin, credential and other failures do not retry automatically.

Offline source commits retain bounded per-view history under the immutable local approval lifetime. Successful same-context resume replays that history through the existing reducer. History loss installs a higher-revision checkpoint through the same reducer, preserving retirement and clock checks. Changed limits/profiles/policy or an authenticated `ClientHello.fresh_view` request select a distinct view before the old replica is disposed. The client rejects incompatible limits under an unchanged context. Reconnection never restores a lease, held input, composition or presentation: acknowledge the recovered state and complete the renderer or semantic-readiness callback before acquiring control again. A caller can retry its exact previous command bytes or query its sequence; recovery never redispatches a consumed command.

Zero-delta resume retains the exact source closure atomically with negotiation. Cache loss can still require verified resource delivery before APPLIED is resent. Revoking a local approval permanently invalidates its lifetime token, including disconnected retention, even if the same principal and permissions are immediately approved again. Newly requested reads/publications fail after revocation; already-held local captures or previously delivered bytes cannot be recalled.

The negotiated optional profile `semantics.only.v1` selects `OpenView {selection:"fixture", mirror_canvas_policy:"semantic-only"}`. This projection carries surface topology, semantic/text data and source focus, with no Scene/Binding records, visual references, cursor or resource closure. Surface visual coverage is explicitly Unsupported. After validating and acknowledging applied state, the semantic client sends `SemanticReady` (kind 35, `RevisionAck` payload). This readiness permits only the semantic command family; it cannot authorize raw input, composition or resize. The server rejects PresentedAck for this scope, and reconnect/resource loss clears semantic readiness. Mirror views continue to require a real renderer presentation callback.

A semantic-only receiver validates transmitted Surface geometry structurally, without rendering it. Command discovery uses Semantic actions, Text scalar versions/ranges and stable Collection item IDs. No Scene, Binding, resource or Cursor content belongs to this profile.

`NetworkClient::receive` is cancellation-safe for staged/applied state and returned events. It handles a packet synchronously through the event return and queues required acknowledgements before returning. The next receive/send flushes them. Each preencoded packet remains at the queue front until its flush succeeds. Sink admission is recorded before awaiting the flush, so cancellation retains the pending frame without admitting it twice. A cancelled send also retains its queued packet until a later flush or client drop. This queue has an eight-packet count ceiling and the negotiated outbound byte ceiling. GUI workers may select between this receive future and a bounded local command channel.

Outgoing buffering shares `ViewReceiver`'s memory ledger. Before any post-negotiation send, the client reserves lifetime writer headroom of `4 * envelope_bytes + 64 KiB` for bounded framing/TLS send buffers. Each queued packet reserves `4 * encoded_packet_bytes + 4096` before creating its typed payload, wrapping or encoding it; low-level `send_packet` accepts a caller-owned Packet and reserves before its encoding. Encoded bodies use shared immutable bytes, so flushing does not clone their contents. Packet reservations survive cancellation and release after a successful flush or drop; writer headroom remains until client drop. These conservative charges can reject a negotiated view budget too small for its envelope ceiling. The duplex regression polls a blocked flush, cancels it, resumes exactly one frame and verifies all reservations release.

`ServerConfig::diagnostics` optionally installs a trusted local `Diagnostics`
owner. Choose `RecordingPolicy::MetadataOnly` for ordinary diagnostics: entries
contain only direction, kind, length and digest. Exact packet bytes require the
explicit `AuthorizedDiagnosticBytes` policy; a peer cannot enable it. Neither
ClientHello nor ServerHello enters the recorder. Incoming packets are recorded
only after their binding and operation authorization/validation succeed.
Outgoing records contain the exact encoded bytes passed to the WebSocket/TLS
sink, before encryption; they prove an attempted send, not remote receipt.

Each recorder has count/message/byte limits and a complete reserved sub-arena
charged to that host view before construction. A bounded local registry holds
at most the configured number of connections. Exhaustion fails the diagnostic
connection explicitly; it never silently omits subsequent messages. The local
`inspect` callback borrows entries under the exact session/epoch/principal/view
context. Approval revocation makes reads fail and drops retained state on the
next access; connection completion/cancellation also drops the recorder and its
view reservation. There is no automatic disk export. Explicit local copies made
inside `inspect` have their own retention responsibility and cannot be recalled.

The real TLS diagnostic tests cover resource-bearing snapshots/deltas, command
bytes/results, both directions, credential/private-source exclusion, metadata
mode, wrong scope, revocation, connection capacity and record exhaustion. These
are recorder integration evidence. The controlled native two-mirror interaction
now has a separate [exact recorded network audit](../docs/desktop-controls.md),
including full record prefixes and independently verified vector bodies. Real GTK
producer wire composition and native session input remain separate gates.

`tests/fault_matrix.rs` learns the bounded actual packet sequence, then closes
owned TLS/WebSocket connections before every snapshot/delta begin, publication
chunk/commit, resource offer/chunk/completion and first command-result boundary.
The accepted baseline has 34 cold snapshot/resource boundaries and four warm
command/delta boundaries. Each cut preserves the complete prior record map until
commit, clears staging on retention, reconnects through the real server and
compares recovered records and CPU pixels with a direct authoritative capture.
The pixel comparison covers the entire 920×480 source canvas at half scale.
Lost command replies retain the same exact result without a second source effect;
a fresh command using the old lease is rejected. Separate delayed transfers also
converge. Receiver allocations return to zero after close.

The fault relay uses two private normally verified TLS legs, the exact fixture
certificate, maintained bounded WebSocket framing and exact forwarded packets.
It injects application-level connection cuts, not datagram loss. Its vector font
is deliberately synthetic and resource-bearing; native glyph fidelity and real
GTK Presented callbacks are separate gates. This suite supplies transport/core
and CPU render recovery evidence, not native GUI recovery or P1 measurements.
