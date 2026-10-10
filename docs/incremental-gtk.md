# Incremental GTK publications

Warm publications are transactions. Each authorized view owns a canonical
projection and the source clocks from its capture. Record clocks advance once
when canonical content changes; text, collection and focus clocks obey the
reducer's separate transition rules. Source clock jumps are retained for native
dispatch, never copied into transaction clocks. Every GTK command names the
current public revision, so an older command cannot be rebound to newer native
clocks after a coalesced away/back change. Exact ledger retries retain their
original result and never dispatch again.

Before emission, transaction and snapshot candidates run through independent
bounded forks of the same predecessor, including permanent retirements. Both
must commit with the full resource closure and agree on every field, retirements
and resources. Fork allocations share the receiver's existing memory ledger.
The public canonical projection, unencoded latest target, native clock map,
encoding scratch and retained immutable resource bodies carry view reservations.
Existing private capture (1 MiB), private consumer (8 MiB), public view (64 MiB),
record and lifetime ceilings remain unchanged. Exhaustion closes only that view.

A delayed view retains one replaceable unencoded target. Once ready, it emits
base + 1, even if several source captures intervened. Source capture revisions
and per-view revisions are separate. Exact broker queue/history bytes are never
rewritten. Resource bodies with the same verified digest share ownership across
closures. Broker history and active transfer witnesses pin those bodies until
acknowledgement/history eviction, snapshot replacement, revocation or disposal
releases the last owner. The transport requests only unavailable resources.

Initial connection, source replacement and unavailable/recovery boundaries still
use snapshots. Disconnect destroys the GTK view; GTK retained-delta resume is
unsupported. View readiness clears on installation and requires current Applied
plus Presented or SemanticReady. A partial visual update cannot reuse an older
frame's control authority. Mirror draft capture likewise waits for current
presentation, not just application. A bounded single-command wait handles both
result-before-frame and frame-before-result ordering; receiving a Focus result
alone cannot unlock an edit from its preceding capture. Unknown outcomes remain
blocked under the exact command ledger.

Whole publication limits differ from socket message limits. A locally retained
envelope is decoded through `receive_publication_envelope`; the assembler and
reducer still enforce negotiated snapshot/transaction and memory ceilings.
`receive_envelope` retains the negotiated single-message limit. Network delivery
uses existing bounded begin/chunk/commit messages. A regression emits a 72 KiB
one-field transaction with a 32 KiB message ceiling and no unchanged scene tree.
An 80 KiB transaction tests abort/retry at every chunk cut without exposing a
partial state.

Validation includes coalesced all-record native replay, text/selection clocks,
missing resources, retirement, cache loss with a closed source cache, stalled
independent views, partial visual updates, allocation failure, restart, native
focus/selection ABA, Unicode, actual TLS composition and headed production HTTPS
browser interaction. The [authorized semantic trace](../fixtures/gtk/incremental-publications.json)
has independent Python and Rust full-field agreement at every prefix; historical
acceptance corpora are unchanged. Generate native evidence on an owned display,
then run `python3 fixtures/gtk/incremental_check.py` after building the core replay
example. These results cover the pinned cooperating GTK source, not arbitrary
producer permissions or retained GTK reconnect histories.
