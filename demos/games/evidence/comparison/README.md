# Paired comparison evidence

The [report](../../../../docs/game-vector-raster-comparison.md) analyzes these measurements.
The [benchmark guide](../../bench/README.md) explains how to replay or recapture them.

Each game has one 200-frame native capture at 25 Hz.
The native clips were sampled once; each resolution has three browser processing runs and each encoding has three repeated encoder runs.
No ROM, credential, or user session content is included.

| File | Content |
| --- | --- |
| `summary.json` | Combined rates, processing percentiles, quality, and labeled network models |
| `tables.md` | Tables generated directly from the summary |
| `bandwidth.*`, `client-cost.*` | Plots of measured payload rates and client processing |
| `quality-detail.png`, `zoom-detail.png` | Enlarged quality and local-scaling examples |
| `GAME/capture.json` | Native capture counts, schedule, MAME version, and trace hash |
| `GAME/frames.jsonl.gz` | Compressed native beam trace for reproducible paired replay |
| `GAME/publications.rui.gz` | Exact core envelopes with four-byte local file length prefixes |
| `GAME/publications.json` | Per-frame byte counts, changed-scene contribution, and projector/builder/encoder timing |
| `GAME/render.json.gz` | Three full browser receiver/export/draw sample sets for each output size, plus reference pixel hashes |
| `GAME/codecs.json` | Encoded sizes, all encoder runs, decode CPU work, exactness/error checks, commands, and compression estimate |
| `GAME/decode.json.gz` | Three full browser PNG/H.264 decoder/blit sample sets for each supported output size |

The gzip files are repository packaging, not the on-wire encoding used by the demo.
The separate zlib estimate compresses each core publication independently and is not a shipped feature.
The RUI visual trace omits semantic input controls and session chunk/acknowledgement traffic.
Raster payloads omit media transport and their prospective control channel.
Timing populations and exclusions are specified in the report and script guide.
The full uncompressed pixels and encoded media streams are reproducible outputs under `/tmp/rui-comparison` on the measurement machine.
