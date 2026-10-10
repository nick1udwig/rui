# Controlled headless host fixture

`rui-host-broker::ControlledHost` owns a cooperating application, session broker, command ledger, and per-client projections.
Its application has an increment button and a private editable text field.
This is an in-memory integration fixture; it supplies no network authentication, GTK capture, renderer, or OS input injection.

The trusted adapter supplies `VerifiedAuthorization`.
Negotiation creates an authorized view, then `checkpoint` installs a coherent capture and returns an exact bound Snapshot envelope.
A client can feed that envelope into `ViewReceiver`, acknowledge application, and acknowledge presentation after its renderer completes.
The fixture's tests simulate that last callback.
A control lease can be acquired only after those readiness checks.

`submit` is the mandatory semantic dispatch boundary.
It admits the exact command bytes through the deduplication ledger, checks the current lease and permissions, validates relevant source versions, and executes the application operation.
The application revalidates before mutation.
Successful dispatch returns source commit evidence; each connected view receives its own dependent Transaction envelope.
Failure to publish to one client closes that view without rolling back an already completed application operation or blocking healthy clients.
A result names a resulting view revision only if that revision was successfully published.

`Activate` changes the authoritative counter.
`ReplaceText` uses Unicode scalar ranges and requires the relevant text and focus generations.
A repeated command has one effect; stale or disabled targets fail at dispatch.
An accepted text replacement with unchanged content preserves the text clock while recording a source capture commit.
Local activity, disconnect, and lease expiry revoke control through the session broker.
Input cleanup is returned as adapter work.

Private content requires the explicit trusted action permission `fixture.read-secret.v1`; control permission alone does not grant it.
Views without that permission receive redacted text, no private text action, and a constant public vector rectangle.
Private strings and their length do not influence that rectangle.
Each view has separately encoded publications and history.
Public diagnostics contain counts and clock values, never private content.
This establishes a cooperating fixture's redaction behavior, not a general redaction pipeline for arbitrary applications.

`snapshot` reads the current published capture without clearing history or acknowledgements.
It is suitable for inspection or a fresh replica.
An existing replica must not substitute it for a transaction at the same revision: exact payload identity is part of replay validation.
Use `checkpoint` for explicit recovery; it advances the view revision and resets queue, history, and readiness.
If the source changes while a client is disconnected, that client's projection requires a fresh checkpoint so skipped object versions cannot enter a delta.

Snapshot handles and transaction handles retain their broker memory reservation through the final clone.
The fixture has a fixed record count and a bounded source text field.
It emits complete envelopes within the negotiated ceiling; it does not yet produce chunked publications.
Real transports must bound their own buffers, authenticate reconnections, and translate lifecycle and renderer events into these APIs.
