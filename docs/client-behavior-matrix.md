# Native and portable session behavior

L2's first migration slice extracts `rui-wire`; schema, codec/preflight, errors,
redaction and zeroizing ownership are shared without a dependency cycle. The
native networking actor is still `NetworkClient`. This matrix controls subsequent
migration; unsupported portable behavior must be explicit before removing native code.

| Behavior | Native transport | Portable actor at this slice |
| --- | --- | --- |
| Wire versions/profiles/permissions | caller hello, negotiated subset, typed grants | fixed browser subset, permission gate; configuration needed for semantic-only |
| View selection | explicit selection and canvas policy | explicit selection, host-canvas policy |
| Readiness | Applied, actual Presented or SemanticReady | Applied and actual Presented; SemanticReady pending S1 |
| Publication/chunk assembly | Rust ViewReceiver | same Rust ViewReceiver and exact publication hashes |
| Resources | bounded paging, eviction/cache loss, retained pins | verified offers/bodies, bounded initial requests; eviction/cache-loss API gap |
| Resume/reset | retained receiver with caller hello, changed scope fallback | retained receiver and exact pending command queries, same-scope limits/profile check |
| Semantic commands | shared builders; exact bytes | shared builders, activation/focus/text/selection only |
| Raw input/composition/resize | supported when negotiated and native adapter proves it | builders shared; no portable capability claim |
| Leases | native integration manages request IDs, held-input cleanup | actor manages acquire/renew/release and stale-response fencing |
| Queued sends | charged sink admission, submitted marker across cancelled await | bounded charged synchronous queue; generation fencing in browser |
| Command ambiguity | common host ledger, native UI exact pending queue | common host ledger, retained exact pending queue; no fresh retry |
| Disposal | receiver, sink/writer allocations and GUI cleanup | receiver/commands cleared explicitly; browser clears derived pixels/state |

Ownership contract: transport admission retains encoded bytes and a shared-budget
reservation until successful flush. A cancelled await after sink admission keeps
the submitted marker and resumes that exact frame once. Portable synchronous
admission removes actor bytes only after a platform transport takes/copies them;
browser buffered-byte limits and connection generations remain platform duties.
Neither path declares presentation from packet receipt or Applied.

The native queue remains because its cancellation and all command families are
already proven. Full `NetworkClient` migration is outstanding and is not implied
by the crate extraction. The plan allows semantic session/library work to proceed
before that migration; existing native integration gates must pass before duplicate
actor behavior can be deleted. Protocol transcripts and codec rejection tests
remain unchanged across this slice.
