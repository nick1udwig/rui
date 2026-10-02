# Implementation acceptance

The original root `conformance.yaml` remains an unchanged design artifact. Executable test coverage is tracked here separately so that implementation progress cannot be mistaken for completion of the entire RFC.

## Implemented core slice

The first implementation covers the transport-independent M1 foundation: generated Protobuf vocabulary, atomic replica validation, bounded chunk assembly, strict vector resources, command deduplication, and session authority. Test names refer to specification case IDs where applicable; a test covering a core invariant does not complete an integration or rendering case.

| Area | Executable evidence | Boundary |
|---|---|---|
| Schema and reducer | `protocol-core/tests/model.rs`, independent Python replay | Initial vocabulary only; no renderer |
| Exact-byte publication assembly | `protocol-core/tests/publication.rs` | In-memory chunks; no WebSocket binding |
| Resources and lifecycle | `protocol-core/tests/resources.rs` | Limited safe vector format; trusted producer provenance |
| Deduplication and typed commands | `protocol-core/tests/commands.rs` | Synthetic source callbacks; no application adapter |
| Negotiation, recovery, leases, queues | `protocol-core/tests/session.rs` | Authenticated identity supplied by caller; no TLS/OS input |
| Receiver integration | `protocol-core/tests/pipeline.rs` | Shared budgets, resource pins, staged/atomic/presented boundaries |
| Independent decoder/reducer | `python3 conformance/check.py` | Semantic-only subset; 16 deterministic seeds and checked-in exact golden bytes |

Review corrections include cleanup of abandoned resource transfers, poisoning conflicting duplicate chunks, binding client identities to principals, rejecting revision gaps, requiring presentation before input, charging queue bookkeeping, retaining dispatch keys across expired outcomes and callback panics, and checking content-version clocks.

Local validation: **95 Rust tests pass**, and the independent Python/Rust check passes **1,275 prefixes** across exact golden fixtures, 16 seeds, and equivalent fresh snapshots. These are implementation tests, not 95 completed cases from the original specification. Formatting and Clippy with warnings denied also pass locally; the included CI workflow runs the same checks. The suite includes a 10,000-record snapshot/update and hostile repeated-string-field capacity accounting.

## Gates still open

- M0: capture a representative corpus using a pinned GTK build, demonstrate actual shared capture boundaries, and publish measured supported/unsupported node coverage. Availability of GTK on a development machine is not this evidence.
- M1: complete the full proposed vocabulary and all core cases, connect all resource/session/reducer budgets, and extend independent interoperability and property testing across that vocabulary.
- M2: authoritative graphical fixture, two desktop clients, renderer comparisons, reconnects, and resource-cache recovery.
- M3–M5: real GTK capture, compositor/portal integration, production hardening, and the semantic-only application client.

No TLS authentication, user approval UI, renderer, OS input injection, source-integrated application, or real secret-redaction pipeline is supplied by these core data structures. Callers must establish authenticated identities and producer evidence at their own boundaries.
