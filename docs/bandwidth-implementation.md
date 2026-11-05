# Bandwidth implementation results

## Independent publication compression

The optional [Zstandard chunk profile](../spec/compression-profile.md) is implemented in the native sender and shared native/WASM receiver.
New clients and default host configurations negotiate it automatically; older peers retain their existing encoding.
This first step does not require changing application adapters or renderer primitives.

The production encoder was replayed over the 200-publication traces in the [game comparison](game-vector-raster-comparison.md).
Counts include session publication begin, chunk, and commit packets, excluding authentication, acknowledgements, and TLS/WebSocket framing.
Each trace represents eight seconds at 25 Hz.

| Trace | Original packet bytes | Compressed packet bytes | Reduction | Compressed Mb/s |
| --- | ---: | ---: | ---: | ---: |
| Battlezone | 3,767,190 | 748,466 | 80.1% | 0.748 |
| Asteroids | 1,745,698 | 491,764 | 71.8% | 0.492 |

All 400 publications produced exactly the same state in the built WASM session receiver as direct replay of the original core envelopes.
Both receivers released their accounted memory on close.
The transport suite also passes with the pinned Cairo renderer, including live TLS faults at compressed publication boundaries and reconnects.
Tests cover negotiation rejection, output limits, truncation, trailing bytes, and ordinary encoding for data that does not shrink.

This is independent chunk compression, not the temporal Zstandard experiment from the [directions report](rui-bandwidth-directions.md).
The exact graphics and existing rendering cost are unchanged.
The native-resolution H.264 payloads in the earlier comparison remain smaller at this stage.

Reproduce the production encoder and built WASM checks from the repository root:

```sh
cargo build --release --locked -p rui-transport --example compression_probe
python3 client-wasm/build.py
gzip -dc demos/games/evidence/comparison/bzone/publications.rui.gz > /tmp/rui-bzone.rui
target/release/examples/compression_probe /tmp/rui-bzone.rui /tmp/rui-bzone.packets
node demos/games/bench/check-compression.mjs demos/games/evidence/comparison/bzone/publications.rui.gz /tmp/rui-bzone.packets
```

Repeat with `asteroid` for the other trace.
