All rates are visual-payload Mb/s at 25 workload frames/s.

| Game | Output | Raw RGB24 | RUI | PNG | H.264 RGB lossless | H.264 CRF18 | H.264 CRF28 UF |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Battlezone | 320 × 240 | 46.080 | 3.714 | 2.080 | 0.491 | 0.155 | 0.162 |
| Battlezone | 640 × 480 | 184.320 | 3.714 | 5.000 | 1.139 | 0.323 | 0.373 |
| Battlezone | 1280 × 960 | 737.280 | 3.714 | 12.189 | 2.386 | 0.668 | 0.872 |
| Asteroids | 320 × 240 | 46.080 | 1.693 | 1.071 | 0.502 | 0.120 | 0.128 |
| Asteroids | 640 × 480 | 184.320 | 1.693 | 2.548 | 1.038 | 0.217 | 0.252 |
| Asteroids | 1280 × 960 | 737.280 | 1.693 | 6.558 | 2.182 | 0.439 | 0.585 |

First visual payload at 640 × 480, excluding authentication and browser asset loading.

| Game | RUI snapshot bytes | PNG first image bytes | Lossless RGB H.264 keyframe bytes | CRF18 H.264 keyframe bytes |
| --- | ---: | ---: | ---: | ---: |
| Battlezone | 37,528 | 23,108 | 21,081 | 9,593 |
| Asteroids | 14,611 | 9,288 | 6,195 | 2,536 |

H.264 lossless and CRF18 columns use `veryfast`; CRF28 UF uses `ultrafast`.
All retained encodings, including additional ultrafast cases, are listed in summary JSON.

Warm browser wall time, p95 milliseconds per frame.

| Game | Output | RUI total | RUI draw only | PNG decode/blit | H.264 CRF18 decode/blit |
| --- | --- | ---: | ---: | ---: | ---: |
| Battlezone | 320 × 240 | 11.30 | 9.60 | 1.20 | 0.50 |
| Battlezone | 640 × 480 | 17.40 | 15.80 | 2.90 | 2.00 |
| Battlezone | 1280 × 960 | 44.70 | 42.90 | 9.00 | 5.20 |
| Asteroids | 320 × 240 | 6.10 | 5.50 | 1.20 | 0.30 |
| Asteroids | 640 × 480 | 14.30 | 13.60 | 2.70 | 1.60 |
| Asteroids | 1280 × 960 | 40.30 | 39.70 | 8.70 | 4.80 |

Host stage measurements at 640 × 480.

| Game | RUI project/build/encode wall p95 | PNG CPU/frame | H.264 RGB lossless CPU/frame | H.264 CRF18 CPU/frame |
| --- | ---: | ---: | ---: | ---: |
| Battlezone | 0.189 ms | 3.292 ms | 3.476 ms | 2.267 ms |
| Asteroids | 0.094 ms | 3.172 ms | 3.472 ms | 2.165 ms |

Native-resolution quality; foreground error uses 8-bit RGB channel values.

| Game | Encoding | Exact pixels | Full-frame PSNR, dB | Foreground RMSE | Foreground channels with error >8 |
| --- | --- | --- | ---: | ---: | ---: |
| Battlezone | png | Yes | ∞ | 0.00 | 0.0% |
| Battlezone | h264-lossless-rgb-veryfast | Yes | ∞ | 0.00 | 0.0% |
| Battlezone | h264-crf18-veryfast | No | 44.18 | 7.16 | 20.9% |
| Battlezone | h264-crf28 | No | 43.47 | 7.68 | 23.6% |
| Asteroids | png | Yes | ∞ | 0.00 | 0.0% |
| Asteroids | h264-lossless-rgb-veryfast | Yes | ∞ | 0.00 | 0.0% |
| Asteroids | h264-crf18-veryfast | No | 60.21 | 1.93 | 0.3% |
| Asteroids | h264-crf28 | No | 57.26 | 2.69 | 1.5% |

Modeled native-resolution Battlezone serialization time per average frame.

| Link capacity | RUI | Lossless RGB H.264 | CRF18 H.264 | Mean RUI load fits | Mean CRF18 load fits |
| --- | ---: | ---: | ---: | --- | --- |
| 0.256 Mb/s | 580.3 ms | 177.9 ms | 50.5 ms | No | No |
| 1 Mb/s | 148.6 ms | 45.6 ms | 12.9 ms | No | Yes |
| 5 Mb/s | 29.7 ms | 9.1 ms | 2.6 ms | Yes | Yes |

Battlezone: scene mutations account for **97.1%** of warm publication bytes.
Independent zlib level 6 would reduce payload from **3.714 to 0.687 Mb/s**, a **5.4×** reduction, on this exact trace.
Its measured Python compression-call p95 was **0.424 ms** per publication; browser decompression and integration costs were not measured.

Asteroids: scene mutations account for **95.7%** of warm publication bytes.
Independent zlib level 6 would reduce payload from **1.693 to 0.428 Mb/s**, a **4.0×** reduction, on this exact trace.
Its measured Python compression-call p95 was **0.187 ms** per publication; browser decompression and integration costs were not measured.
