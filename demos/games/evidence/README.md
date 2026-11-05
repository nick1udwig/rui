# Local acceptance

These reports came from the actual MAME → RUI → Chromium path on an Intel i7-1280P.
MAME 0.289 and Chromium 155 ran on owned authenticated Xvfb displays with the loopback TLS/WebSocket proxy.
The native vector canvas was 640 × 480 and the desktop client rendered at scale 0.75 during measurement.
The mobile check used Chromium touch emulation at 390 × 844.

| Game | Warm measurement | Presented frames/s | Successful input acknowledgement p95 | Samples | Received KiB/s |
| --- | ---: | ---: | ---: | ---: | ---: |
| [Battlezone](bzone.json) | 30 s | 25.0 | 30.4 ms | 89 | 231.9 |
| [Asteroids](asteroid.json) | 6 s | 25.2 | 23.3 ms | 19 | 146.4 |

Frame rate counts the browser's following-frame presentation callbacks, not physical scanout.
Input timing starts when the browser submits a guarded command and ends when it receives the successful native `Dispatched` result.
It excludes time waiting locally for presentation readiness and does not measure key-to-photon latency.
Rejected commands are excluded from the input timing population.
Bytes include RUI traffic but exclude authentication setup, TLS, and socket overhead during the warm interval.
Both games passed visible vector coverage, coin/start input, simultaneous controls, zoom, blur/disconnect cleanup, reconnect, and mobile touch input.
Battlezone also passed combined steering keys and release back to forward drive.
These local functional runs establish neither Internet performance nor a bandwidth advantage over video.
