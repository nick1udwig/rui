# Protocol core implementation boundary

The Rust core implements the versioned, toolkit-independent RUI/0.1 vocabulary, reducer, publication/resource ownership, command ledger and session authority. Native producer and desktop integration have separate acceptance gates in [the implementation audit](../conformance/STATUS.md).

## Wire and model

Each binary envelope binds a payload to a protocol version, session, epoch, and view. The current envelope kinds carry a snapshot, transaction, or semantic command. Resource/publication transfer, negotiation, recovery and lease APIs are Rust model APIs. [The transport implementation](../transport/README.md) binds these APIs to authenticated TLS/WebSocket messages. Semantic, raw-input, composition and resize commands share one exact-byte sequence ledger.

The scene vocabulary includes Group, SolidRect, RoundedRect, Border, LinearGradient, RadialGradient, FillPath, StrokePath, and GlyphRun. Their supported geometry and paint rules are defined in [rendering-profile.md](rendering-profile.md). The [CPU reference renderer](renderer-profile.md) implements this vocabulary. Unknown required mutations, record kinds, primitives and features fail explicitly; unsupported native effects remain named coverage gaps.

Geometry is finite and bounded; model decoding normalizes negative zero. Colors are validated as premultiplied channels in the range zero to one. Text selections count Unicode scalar values. No text shaping, reflow, font substitution, or grapheme navigation occurs in the replica.

Snapshot and transaction APIs consume exact received Protobuf payload bytes. Digests cover those bytes, including unknown optional fields; they never cover a reserialization. Full-record replacement clears absent optional fields. Record IDs are globally typed within an epoch, and deleted IDs remain retired. A caller must use a fresh replica for a new epoch/view.

In a transaction, changing text content or availability advances the text clock by exactly one; a selection-only replacement retains that clock. Changes to collection membership, selection, counts, or materialization advance its clock by exactly one. Replacement snapshots may skip intervening clock values but cannot regress them. `COHERENT` capture requires a nonempty producer token; `OBSERVED` requires separate nonempty semantic/visual tokens. The trusted producer remains responsible for the truth of that evidence.

Reusing a remembered publication revision with different exact bytes is a digest conflict. An unknown old revision outside retained history is a revision error. Both leave committed state intact and require the connection layer to recover or close explicitly.

## Receiver integration contract

`receiver::ViewReceiver` owns a private assembler, resource cache, and replica under one shared `MemoryBudget`. Construct it with a trusted session/epoch/principal/view binding and negotiated receiver limits. It accepts exact bound envelopes or the bounded publication chunk API. Local ceilings can only lower the negotiated limits. One publication may be assembling or waiting for resources at a time.

The lower-level APIs remain available. When integrating them directly, use one `MemoryBudget` for the publication assembler, resource cache, and replica. Independent default constructors are useful in unit tests; they do not establish an aggregate receiver ceiling across separately constructed components.

The assembler validates chunk indices, lengths, counts, and the exact digest. `Replica::apply_publication` additionally binds the decoded record/mutation count and revisions to the assembler's validated metadata. Direct `apply_snapshot` and `apply_transaction` are low-level entry points for already assembled payloads.

Resource availability is a trusted input to the low-level reducer. Supply only safely decoded, currently authorized cache entries and locally fingerprint-verified fonts. A fingerprint does not authorize fetching a font file. The cache supports vector resources; font installation and fingerprint verification are not implemented. The coordinated `ViewReceiver` rejects glyphs by default. After explicitly negotiating [the single-outline glyph profile](glyph-outline-profile.md), a compatible renderer may opt in to verified normalized outlines with glyph ID zero. Other glyph mappings and font fingerprints still fail explicitly.

`ViewReceiver` pins the full candidate closure before requesting resources, preserves the old live closure during staging, and promotes staged pins without allocation after successful atomic application. Resource grants remain trusted policy inputs; a peer offer cannot authorize itself. Offers must belong to the live or pending closure and match the resource kind used by that scene. Rejected and abandoned candidates release their staged ownership. Direct users of the lower-level APIs must implement this lifecycle themselves; `protocol-core/tests/pipeline.rs` demonstrates it.

The optional `commit_authenticated_publication` entry point lets a trusted
transport adapter treat a pinned, authenticated producer's validated graph
references as grants for that bound view. The adapter must establish this
authority; no wire flag enables the policy. Exact bytes, graph, features and
font use are validated before granting anything. This entry point rejects extra
`required_resources` dependencies outside the graph; prefetch needs separate
explicit authorization. Failure aborts the candidate and prunes unused grants
while preserving live state and pins. Ordinary `commit_publication` continues
to require prior explicit grants, and neither entry point accepts unsolicited
resource bodies or treats a grant as verified availability.

Pending-resource replies retain allocation reservations until dropped, including after receiver closure. `ViewReceiver` additionally bounds their aggregate bookkeeping by the negotiated outbound queue allowance. A memory-pressure failure while resuming a validated pending publication preserves that publication for a later retry. Transport buffers and network work remain the caller's responsibility.

Presentation acknowledgement must follow actual renderer presentation. `ViewReceiver::mark_presented` accepts only the current committed revision with an available resource closure; the callback remains trusted renderer evidence. Disconnect aborts partial work and clears presentation while retaining verified live state for authorized same-view reconnection. Cache loss clears presentation and requires resource recovery. Close/revoke irreversibly disposes of the view. A network implementation must connect those lifecycle events to authenticated session policy and gate input on presentation and a current control lease.

## Producer allocation ownership

`SessionBroker::with_memory_budget` charges every view reservation against both
its negotiated view limit and a shared producer budget. Publication bytes,
history, attachments and escaping transport reservations retain the parent charge
until their actual owner drops, even after broker shutdown or view replacement.
An attachment destroys its value before releasing either charge. Failed parent or
view admission releases temporary reservations without installing a publication.
The ordinary `SessionBroker::new` retains independent per-view budgets; a host
using it must supply a separate aggregate allocation policy. Fixed broker and
command-ledger metadata still need their own bounded producer allowance.

## Authority and privacy

Authentication, projection policy, and application completion evidence are trusted host inputs. A struct named `VerifiedAuthorization` is not a credential validator. Connect it to real authentication and authorization before exposing a network endpoint.

Command admission binds the authenticated principal/producer to the exact Protobuf payload. Sequence and identity retention deliberately fail closed at configured exhaustion; discarding the ledger requires a new epoch. Dispatch callbacks must validate source state at the application boundary. Source completion evidence must come from the application rather than inferred timing.

The session broker emits input-cleanup work for an adapter. It does not inject or release operating-system input itself. The host/client connection layer coordinates resource revocation and replica disposal with broker view invalidation; native adapters must also complete their source cleanup obligations.

The [controlled headless host](host-fixture.md) joins session authority, command admission, and source validation behind a single dispatch API. It independently projects semantic, text, and scene records for each view and tests observer redaction across snapshots, updates, and retained recovery history. That privacy behavior applies to its fixed cooperating application. The transport provides bounded opt-in, scoped diagnostic recording. This does not establish arbitrary-application redaction. Resource provenance is an assertion made by a cooperating producer; a vector decoder cannot prove that an untrusted producer did not derive geometry from raster pixels.
