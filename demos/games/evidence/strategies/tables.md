# Offline bandwidth strategy probes

Payload Mb/s on the retained eight-second, 25 Hz game traces.

| Representation | Battlezone | Asteroids |
| --- | ---: | ---: |
| Current RUI | 3.714 | 1.693 |
| transport: zlib6-reset1 | 0.687 | 0.428 |
| transport: zlib6-reset25 | 0.577 | 0.267 |
| transport: zlib6-reset0 | 0.569 | 0.256 |
| transport: zstd3-reset1 | 0.687 | 0.431 |
| transport: zstd1-reset25 | 0.354 | 0.235 |
| transport: zstd3-reset25 | 0.336 | 0.220 |
| transport: zstd6-reset25 | 0.317 | 0.201 |
| transport: zstd3-reset0 | 0.303 | 0.192 |
| beam_source: f64 | 0.122 | 0.095 |
| beam_source: decimal-delta | 0.155 | 0.052 |
| beam_source: byte-shuffle | 0.443 | 0.362 |
| H.264 CRF18 veryfast, width 320 | 0.155 | 0.120 |
| H.264 CRF18 veryfast, width 640 | 0.323 | 0.217 |
| H.264 CRF18 veryfast, width 1280 | 0.668 | 0.439 |

Transport rows reconstruct complete core publications.
Beam rows reconstruct source geometry and omit protocol records.
Every update is flushed immediately.
Zstandard history is capped at 256 KiB; reset0 keeps this bounded history for the full clip.
Reset25 starts new compression history every second; it is not a scene recovery checkpoint.
Byte shuffling is retained as a negative result.
