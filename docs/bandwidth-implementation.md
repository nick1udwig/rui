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

## Bounded history compression

The optional history extension uses the last 256 KiB of decoded publication data as a dictionary.
The sender chooses the smallest of ordinary, independent Zstandard, and history Zstandard encoding for each chunk.
Every history packet includes a digest of the required base; reconnects and snapshots reset history.

| Trace | Original packet bytes | History packet bytes | Reduction | History Mb/s |
| --- | ---: | ---: | ---: | ---: |
| Battlezone | 3,767,190 | 371,588 | 90.1% | 0.372 |
| Asteroids | 1,745,698 | 271,219 | 84.5% | 0.271 |

These counts use the same complete publication-packet boundary as the independent results.
All 400 decoded WASM states match direct original-envelope replay exactly.
The history path still uses more bytes than the earlier native-resolution H.264 payload, while using fewer than its 1280 × 960 payload.
This comparison does not equalize media transport or recovery traffic.

Pass `history` as the final argument to `compression_probe` to exercise this path.
The WASM check command is unchanged.
The regression suite injects connection loss at both compressed chunk kinds and checks reconstruction of authoritative state and rendered pixels.
Diagnostic privacy checks now inspect decoded publication bodies, including history-compressed data.

## Compact ordered geometry

The [segment profile](../spec/segments-profile.md) carries ordered float64 endpoints and colors in one scene record.
The game producer now uses this profile; the browser and native renderers implement it.
No game simulation moves to the client, and no coordinate or color quantization is used.
The general primitive also applies to charts, diagrams, and wireframes.
Other application producers retain their existing representations and benefit from transport compression alone.

| Trace | Segment packet bytes, plain | Independent Zstandard | History Zstandard | History Mb/s | Native H.264 Mb/s |
| --- | ---: | ---: | ---: | ---: | ---: |
| Battlezone | 2,275,412 | 467,309 | 176,308 | 0.176 | 0.323 |
| Asteroids | 1,514,772 | 369,171 | 174,647 | 0.175 | 0.217 |

The combined changes reduce publication packet bytes by 95.3% for Battlezone and 90.0% for Asteroids against the original session encoding.
They use 45.5% and 19.6% fewer bytes than the earlier native-resolution H.264 CRF18 veryfast payloads.
These are eight-second retained workload results, not a claim of superiority over every H.264 setting or every application.
The RUI counts include publication metadata, but exclude handshake, acknowledgements, controls, and TLS/WebSocket framing.
The H.264 counts exclude equivalent network framing and controls.
At 320 × 240, the earlier H.264 payloads remain smaller than this RUI stream.
The segment stream is independent of display resolution.

All 1,200 frame renders are byte-identical to the retained original pixels: 400 frames at each of 320 × 240, 640 × 480, and 1280 × 960.
Both independent and history compression pass exact WASM state checks for every publication.
The [retained results](../demos/games/evidence/segments/summary.json) include hashes, memory accounting, and decode timing.
In one desktop Node/WASM replay, history receive-and-acknowledge p95 was 1.50 ms for Battlezone and 1.30 ms for Asteroids.
This excludes rendering, state export, network latency, and input handling; it is not a phone result.
The history decoder reserves more memory and uses more CPU than independent compression.
The rendering runs overlapped regression work, so their timing samples do not establish a rendering speed improvement.

Core, producer, native renderer, transport, browser conformance, and installed-package checks pass.
These include segment preflight bounds, blending, clipping, degenerate geometry, memory accounting, and the existing TLS recovery fault matrix.

The browser package and Rust crates advance to 0.2.0 because the public primitive union gains a variant.
Update exhaustive primitive switches and deploy matching JavaScript and WASM assets together.
Older clients reject the new game producer's required profile during negotiation.
The historical `game-trace --legacy` export remains available for original evidence reproduction.

Reproduce the production path with:

```sh
cargo build --release --locked -p rui-browser-host --bin game-trace -p rui-transport --example compression_probe
python3 client-wasm/build.py
npm --prefix client-browser run build
python3 demos/games/bench/check-segments.py --output /tmp/rui-segments --render
```

The script checks both compression modes through the built WASM session receiver and requires every committed state to match direct core replay.
With `--render`, it renders all 400 frames at three resolutions and compares their hashes to the retained original pixels.
It records source, publication, renderer, and WASM hashes with byte counts in `summary.json`.
Rendering needs Chromium and a local listener.
Without `--render`, byte and WASM state checks still run; existing render results in the output directory are checked when present.
