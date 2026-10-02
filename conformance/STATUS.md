# Implementation acceptance

The original root `conformance.yaml` remains an unchanged design artifact. Executable test coverage is tracked here separately so that implementation progress cannot be mistaken for completion of the entire RFC.

## Implemented core slice

The implementation covers a transport-independent M1 foundation and a controlled headless host fixture: generated Protobuf vocabulary, atomic replica validation, bounded chunk assembly, strict vector resources, coordinated reception, command deduplication, and session authority. Test names refer to specification case IDs where applicable; a test covering a core invariant does not complete an integration or rendering case.

| Area | Executable evidence | Boundary |
|---|---|---|
| Schema and reducer | `protocol-core/tests/model.rs`, `protocol-core/tests/scene.rs`, independent Python replay | Nine retained scene primitives; no renderer |
| Exact-byte publication assembly | `protocol-core/tests/publication.rs` | In-memory chunks; no WebSocket binding |
| Resources and lifecycle | `protocol-core/tests/resources.rs` | Limited safe vector format; trusted producer provenance |
| Deduplication and typed commands | `protocol-core/tests/commands.rs` | Synthetic source callbacks; no application adapter |
| Negotiation, recovery, leases, queues | `protocol-core/tests/session.rs` | Authenticated identity supplied by caller; no TLS/OS input |
| Receiver integration | `protocol-core/tests/receiver.rs`, `protocol-core/tests/pipeline.rs` | Owned resource pins and shared budgets; cache/disconnect/revocation recovery; renderer callback trusted |
| Controlled host | `host-broker/tests/controlled.rs` | Two headless clients, source dispatch, observer redaction; no GUI or real authentication |
| Independent decoder/reducer | `python3 conformance/check.py` | Semantic replay plus all nine scene variants; 16 deterministic seeds and checked-in exact golden bytes |

Review corrections include cleanup of abandoned resource transfers, poisoning conflicting duplicate chunks, binding client identities to principals, rejecting revision gaps, requiring presentation before input, charging queue bookkeeping, retaining dispatch keys across expired outcomes and callback panics, and checking content-version clocks.

Local validation: **145 Rust tests pass**. The independent Python/Rust check passes **1,275 semantic replay prefixes** and **144 scene fixtures** across exact golden bytes and 16 deterministic seeds. These are implementation tests, not completed cases from the original specification. Formatting and Clippy with warnings denied also pass locally; the included CI workflow runs the same checks. The suite includes a 10,000-record snapshot/update, hostile repeated-field allocation checks, receiver lifecycle recovery, and two-client source dispatch. The runnable headless example produces one increment and one consumed command despite retry.

## Gates still open

- M0: capture a representative corpus using a pinned GTK build, demonstrate actual shared capture boundaries, and publish measured supported/unsupported node coverage. Availability of GTK on a development machine is not this evidence.
- M1: complete the full proposed message vocabulary and all core cases, extend independent interoperability and property testing, and bind transport/session/receiver lifecycles. Glyph verification and outline mapping remain unsupported by the coordinated receiver.
- M2: authoritative graphical fixture, two desktop clients, renderer comparisons, reconnects, and resource-cache recovery.
- M3–M5: real GTK capture, compositor/portal integration, production hardening, and the semantic-only application client.

No TLS authentication, user approval UI, renderer, OS input injection, or arbitrary-application redaction pipeline is supplied. The cooperating headless fixture supplies source evidence and per-view redaction for its own fixed application. Callers must establish authenticated identities and real renderer/producer evidence at their own boundaries.
