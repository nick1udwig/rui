# Independent implementations and replay

`reference.py` and `scene_reference.py` decode exact Protobuf bytes without Rust
or generated bindings. `python3 conformance/check.py` compares 1,275 semantic
prefixes and 144 scene fixtures. A mismatch is reduced by `shrink.py` into
`target/conformance-failure.jsonl`, preserving the admission/state mismatch class
and exact retained operation bytes. Reduction has a 512-run ceiling.

`semantic_client.py` is a second, handwritten standard-library client. It uses
normal certificate/hostname verification plus an exact certificate pin, bounded
uncompressed WebSocket framing, authenticated view bindings, exact publication
hashes, atomic semantic/text reduction and one command sequence. It requires the
`semantics.only.v1` profile and explicitly sends `SemanticReady` after `Applied`.
It never sends `Presented`. The host grants this profile semantic-family control
only; raw input, composition and resize cannot use semantic readiness.

The client discovers advertised actions, text models and materialized collection
items by semantic identity. Surface topology metadata is structurally validated
and discarded for interaction; no scene renderer, visual binding, hit testing or
coordinate-based action selection exists. Text edits use Unicode scalar ranges.
Successful results require source evidence; an explicit exact retry checks that
the retained result is unchanged. Unknown outcomes are never retried as a new
command. The CLI prints counts and command evidence rather than document content
or credentials.

For a locally approved credential from `pair_and_serve`, a counter action is:

```sh
python3 conformance/semantic_client.py --port 8443 \
  --certificate CERTIFICATE.der --credential CREDENTIAL_FILE \
  --instance 03030303030303030303030303030303 --activate-label Increment
```

The optional `--replace-label LABEL --replacement TEXT` and
`--select-item-label LABEL` selectors require unique advertised targets and the
corresponding independently approved permissions. A list item must belong to a
materialized collection; a larger total count remains explicitly partial.

This is a bounded conformance client, not a general desktop product. It supports
the semantic-only subset, conservatively rejects duplicate singular fields,
retains 256 publication digests and at most 20,000 retired identities, and opens
a fresh session per process. Its fixed byte/count ceilings bound work but are not
instrumented Python heap accounting or a proof of process RSS. It does not use
the Rust shared-memory ledger. The production Rust receiver remains responsible
for resource-bearing views and retained reconnect testing.

Validation:

```sh
cargo build -p rui-protocol-core --example replay
python3 -m unittest conformance.test_semantic_client conformance.test_shrink
python3 -m conformance.typed_interop
cargo test -p rui-transport --test independent
```

The typed vectors compare normalized IDs, typed values, text clocks and ranges,
actions, states, collection order/count/materialization/selection and retirement
at every prefix. The actual TLS test invokes the Python CLI against a locally
approved isolated Rust host, confirms one counter effect despite retry and a
Unicode text edit, and verifies zero presentation acknowledgements. Stable-item
selection and convergence with two graphical observers also pass in the accepted
native corpus; combined button/text graphical scenarios remain separately gated.

## Full native record comparison

`python3 -m conformance.full_interop` verifies descriptor-bound vector body hashes,
flat RVP1 geometry and exact envelope hashes before comparing every retained
record field against the Rust reducer. The handwritten decoder covers all six
record kinds and all nine scene primitives. The accepted native corpus currently
exercises 117 publication prefixes across 17 scopes, with five record kinds and
six scene primitives; the existing generated scene fixtures cover all nine.
It includes semantic, Unicode, restart, unsupported-source, whole-view and
observed AT-SPI captures. Each prefix compares retirement and stable object clocks
as well as full values, geometry, glyph positions, resource references, focus,
collections, source tokens and coverage declarations.

This is a bounded positive-trace oracle. It does not replace the core's complete
untrusted-input validation. The replay harness accepts optional fixture resource
digests only after this independent body audit; that input grants no network
authority. `conformance.test_full_reference` checks atomic failure, retirement,
unknown variants, typed bindings and malformed/raster resource rejection.

## Recorded native network interactions

`network_audit.py` consumes explicitly authorized, post-authentication JSONL
diagnostic exports. It independently checks scoped packet bindings, exact byte
lengths and hashes, sequence continuity, retained-prefix boundaries, chunk
assembly, resource descriptors and vector bodies. Every complete publication
is then compared field for field through the handwritten and Rust reducers.
Applied, semantic readiness and Presented require a previously audited complete
publication and verified live resource closure. Exact command retries must retain
their family, bytes and Applied source evidence.

```sh
python3 -m conformance.network_audit /tmp/native.controller.trace.jsonl \
  --replay target/debug/examples/replay --output /tmp/controller-audit.json
```

The bounded positive-trace oracle reports interrupted transfers separately and
claims only the exported prefixes, including explicit reconnect boundaries.
It does not infer physical dispatch or native paint from packet acknowledgements;
the native scenario supplies those source and callback assertions separately.
The auditor excludes credentials and never accepts handshake packets. Inputs
are capped at 128 MiB, 32 segments, 32,768 records, 1 MiB packets, 4 MiB
publications/resources and 512 verified resource identities per scope.
