# Broader structured-workload measurements

RUI beats the tested 640 × 480 H.264 CRF18 payload on all four authored workloads, even after adding its measured TLS traffic in both directions.
This is evidence for specific structured tasks, not universal superiority over video or raster delivery.
CRF28 is smaller for some tasks, and dirty-rectangle PNG is much smaller for the sparse editor updates.

Startup and client work remain important costs.
The browser runtime is about 1.31 MB uncompressed, or 0.335 MB when its files are independently gzip-compressed.
The full-state reference renderer misses the 40 ms frame budget on the text-heavy workloads.

The [harness](../demos/structured/README.md) includes paired pixels, a video quality sweep, lossless checks, cold snapshots, reset recovery, both TLS directions, one-minute state/transport runs, and a portable phone runner.
The phone runner passes a desktop touch-layout check; no physical-phone latency, energy, or Internet-loss result is claimed.

## Workloads and comparison boundary

The four workloads are authored fixtures, not captures of existing desktop applications.
Each paired trace has 300 samples over 12 seconds at 25 Hz, with the final quarter idle.
The editor changes numeric values in text rows; scrolling retains 48 text rows and moves a parent transform.
The chart updates 145 line segments from a deterministic signal.
The diagram retains node shapes and labels while changing positions and connecting edges.
The authored 5×7 font is carried as segment geometry, with semantic text in the same publications.
This stresses fine geometry; it is not a benchmark of the existing glyph-resource profile or native font fidelity.

The Rust exporter uses the production publication builder and checks incremental state against full snapshots.
The built WASM receiver verifies both complete and incremental JavaScript state exports against direct core replay.
Recovery starts a new session replay from a full snapshot halfway through the same timeline.
This measures reset recovery, not efficient resume, random packet loss, or production authorization-service behavior.

The TLS fixture verifies its local certificate and counts encrypted stream bytes in both directions, including setup, WebSocket upgrade/framing, applied acknowledgements, and simulated presented acknowledgements.
It waits for presentation readiness before the next publication; the fixture simulates that readiness without drawing.
Drawing is measured separately in Chromium.
TCP/IP headers, browser asset transfer, authorization-service traffic, and input/control traffic are not included.
There are no inputs or external geometry resources in these authored replays.
The optional 20 ms delivery delay is an application-level experiment, not an RTT/loss emulator.

All video variants encode the exact RUI-rendered frame sequence.
H.264 uses veryfast, zerolatency, no B-frames, one-second keyframes, one thread, and CRF18/CRF28 or lossless RGB CRF0.
The table includes only encoded video payload, not a video transport implementation.
The RUI TLS comparison therefore gives video a transport-overhead advantage, but does not equate recovery policy, startup software, or visual quality.
CRF values do not establish equal perceptual quality across workloads.
Lossy results carry global PSNR only; this can hide foreground defects and does not establish equal text legibility.
Lossless RGB H.264 and dirty-rectangle PNG reconstruct every pixel exactly.

## Native-resolution bandwidth

| Workload | RUI packets Mb/s | TLS both ways Mb/s | H.264 CRF18 Mb/s | H.264 CRF28 Mb/s | Lossless H.264 Mb/s | Dirty PNG Mb/s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| editor | 0.0994 | 0.1500 | 0.2132 | 0.1378 | 0.3356 | 0.0163 |
| scroll | 0.0993 | 0.1500 | 0.3729 | 0.2419 | 0.5989 | 1.4211 |
| chart | 0.3258 | 0.3764 | 0.4175 | 0.1641 | 1.5021 | 2.2524 |
| diagram | 0.1317 | 0.1823 | 0.9046 | 0.2701 | 5.2230 | 4.6755 |

Rates use the 12-second logical workload duration.
The source exporter publishes every sample, including unchanged idle samples, so metadata and acknowledgements impose a floor.
An adapter that suppresses unchanged captures can do better.
The dirty-PNG baseline sends a single changed bounding rectangle with a 16-byte header, and sends nothing for unchanged frames.
Its editor result shows why sparse raster updates must remain a baseline.

At 320 × 240, RUI sends the same geometry and metadata while video can use fewer pixels.
The retained results include that size; native-resolution wins do not imply wins at every display size.
The chart's full float64 coordinates preserve more information than the lower-quality video cases.
That is a candidate for an explicit bounded-error representation, not a reason to label the smaller video invalid.

## Startup, recovery, and client cost

| Workload | Cold bytes, both TLS directions | Reset-recovery cold bytes | Applied round trip p95, ms | Reference render p95 at 640, ms |
| --- | ---: | ---: | ---: | ---: |
| editor | 16,776 | 16,665 | 5.83 | 106.0 |
| scroll | 38,863 | 38,742 | 4.50 | 200.7 |
| chart | 8,914 | 8,929 | 4.78 | 17.5 |
| diagram | 8,527 | 8,550 | 3.69 | 28.0 |

Cold and recovery values end after the first snapshot's simulated presented acknowledgement, including fresh TLS setup.
Recovery snapshots include all retained geometry, so no warm resource cache is assumed.
The runtime asset estimate is separate and is not measured HTTP transfer or a compressed deployment configuration.
A short visit can spend more on cold RUI assets than it saves in scene traffic.
The phone runner serves uncompressed development assets; production caching and compression need separate evaluation.

Reference render timing includes core reduction, full-state export, and Canvas drawing.
It uses one timing run after an export run and is exploratory, not a repeated hardware performance gate.
The full-state exporter matches the earlier game comparison; BrowserClient and the portable runner use incremental exports.
Do not treat reference-render timing as an optimized BrowserClient measurement.
Text-heavy output shows that many tiny primitives can save bandwidth while still being expensive to export and draw.
The wire fixture's acknowledgement timing excludes drawing and is not key-to-photon latency.

## Longer runs and device coverage

| Workload | One-minute RUI TLS Mb/s | WASM accounted peak, MiB |
| --- | ---: | ---: |
| editor | 0.1420 | 4.99 |
| scroll | 0.1297 | 16.84 |
| chart | 0.3722 | 2.86 |
| diagram | 0.1761 | 2.90 |

The longer cases each contain 1,500 source samples over one logical minute, replayed as fast as the receiver permits.
They check state correctness, history operation, memory accounting, and traffic at more revisions.
They are not real-time soak tests or paired one-minute H.264 comparisons.
WASM accounted peaks exclude the JavaScript graph, renderer caches, socket buffers, allocator overhead, and browser process memory.
Each receiver releases its accounted memory on close.

Use the [phone runner instructions](../demos/structured/README.md#browser-and-phone-replay) to collect actual device results over Tailscale.
The retained mobile screenshot is Chromium touch/viewport emulation on the development desktop.
Physical-phone energy, sustained frame rate, thermals, input-to-photon latency, real application captures, and congested/lossy network behavior remain unmeasured.

## Evidence and reproduction

[Primary results](../demos/structured/evidence/summary.json) contain source hashes, packet and TLS counts, codec settings, quality metrics, pixel hashes, and runtime metadata.
[Longer-run results](../demos/structured/evidence/long/summary.json) retain the one-minute state/transport checks.
The fixture directory also retains compressed publications, render samples, reset-recovery results, and representative images.
Use `demos/structured/run.py` to regenerate the comparison and `demos/structured/retain.py` to retain compact evidence.
The supported commands are in the [harness README](../demos/structured/README.md).
