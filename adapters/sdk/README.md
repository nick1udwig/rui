# Cooperating producer SDK

`producer.py` supplies explicit identities, synchronous semantic capture, bounded
vector resources and an exact RUI Protobuf snapshot encoder. The GTK adapter in
`../gtk4/capture.py` supplies the actual source snapshot boundary. This package
does not authenticate a producer, discover authorization, fetch assets or route
network commands.

The Rust `CaptureArtifact` bridge consumes local fixture manifests and their
exact `.bin` envelopes and `.rvp` resources through `ViewReceiver`. Calling
`apply_trusted_fixture` means the caller approved that local producer's resource
policy; reading a JSON manifest is not evidence of trust or actual GTK
provenance. The bridge enables the explicit `glyph.outline-single.v1` profile.
It never acknowledges presentation. A renderer must complete rendering before
calling the receiver's presentation method.

```sh
python3 -m unittest adapters.sdk.test_producer
cargo test -p rui-producer-sdk --locked
cargo run -p rui-producer-sdk --bin replay_capture -- fixtures/gtk/golden
```

The capture manifest has a 1 MiB byte ceiling, depth 64 and a conservative 16 MiB
structural allocation preflight before JSON deserialization. This local artifact
quota is separate from the receiver's shared state/resource budget. Exact
envelopes have a 1 MiB ceiling; each vector resource has a 16 MiB ceiling and is
read separately. Declared resource lengths must equal both file metadata and
the actual body read. These are fixed fixture limits, not a negotiated network
transfer API.

See [the measured GTK coverage report](../../docs/gtk-coverage.md) for the source
scope, pinned environment, local image diagnostics, unsupported rendering and
remaining interaction work.
