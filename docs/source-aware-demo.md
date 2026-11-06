# Source-aware object and camera experiment

This experiment keeps authored object geometry at the client and sends object and camera transforms.
It uses the existing RUI scene model, publication builder, history compression, WASM replica, and Canvas renderer.
It adds no game simulation or application-specific drawing code to the client.

The paired baseline flattens the same scene into a changing compact segment batch.
This separates the benefit of preserving source structure from the earlier benefit of compact beam encoding.
Startup geometry, reset recovery, return traffic, and rendering costs are included in the measurements described below.

The result is an authored 2D replay demo, not an object extractor for the native MAME games or a deployed 3D scene protocol.
The original games expose beams; an actual integration needs a cooperating source or a reliable object/camera capture API.

## Representation

The source owns 32 objects built from three authored outlines, with 16 segments per outline.
Eight objects move; the remaining objects stay fixed in world coordinates.
One camera transform moves the whole world.
Object geometry remains in stable child scene nodes while parent transforms change.
The current model retains a geometry copy per object; it does not introduce a new shared-template resource type.
Zstandard also compresses repeated geometry in the initial snapshot.

Source positions are authored on a quarter-unit grid and shape vertices use integer coordinates.
Both representations use those exact source values.
No representation-specific rounding, interpolation, prediction, or dropped updates is used.
All 300 authoritative samples are delivered, including the final three seconds of idle state.
The browser renders state but does not advance the source simulation or infer motion.
No correction channel is needed because no predictive state is executed on the client.
Reset recovery sends a fresh complete scene halfway through the same timeline.

The retained representation sends camera/object record changes through the existing publication builder.
The flattened representation sends world-space endpoints and colors through `SegmentBatch`.
Paint order, clip, width, color, viewport, and source timeline are the same.
The unit check reconstructs the flattened endpoints from the retained representation and compares every value exactly.

## Results

Over 12 seconds, retained objects reduce publication packet bytes by **82.5%** and bidirectional TLS bytes by **75.3%** against the flattened representation.
This uses the existing protocol and renderer.

| Measurement | Retained objects | Flattened segments |
| --- | ---: | ---: |
| Publication packet bytes | 138,868 | 793,704 |
| Bidirectional TLS bytes | 214,724 | 869,756 |
| Bidirectional TLS Mb/s | 0.1431 | 0.5798 |
| Cold connection through first presentation, TLS bytes | 8,533 | 10,934 |
| Reset connection through first presentation, TLS bytes | 8,580 | 10,943 |
| WASM accounted peak, bytes | 3,076,220 | 3,050,304 |

The retained stream is smaller than both tested lossy H.264 settings at all three resolutions.
Its traffic does not change with local resolution.
At 640 × 480 it uses 90.1% fewer bytes than CRF 18 and 66.5% fewer than CRF 28 under the measurement boundaries below.

| Local resolution | Retained RUI, TLS Mb/s | H.264 CRF 18, payload Mb/s | H.264 CRF 28, payload Mb/s | Lossless H.264, payload Mb/s |
| --- | ---: | ---: | ---: | ---: |
| 320 × 240 | 0.1431 | 0.7430 | 0.2351 | 3.7423 |
| 640 × 480 | 0.1431 | 1.4399 | 0.4274 | 7.0653 |
| 1280 × 960 | 0.1431 | 2.7133 | 0.9798 | 11.1905 |

All **900 frame pairs** match exactly: 300 frames at each of three resolutions.
The checks compare SHA-256 hashes of the actual browser RGBA output, including antialiasing.
All lossless H.264 decodes also match their source pixels.
Both normal and reset streams pass exact WASM state checks.

Rendering remains a separate constraint.
At 640 × 480, the retained reference replay has a 29.0 ms median and 36.2 ms p95 for reduction, full-state export, and drawing; the flattened replay has 32.8 ms and 37.4 ms.
At 1280 × 960, retained p95 reaches 98.0 ms, above the 40 ms budget for 25 Hz.
These are exploratory desktop measurements from one timed run after warmup, not a physical-phone or optimized incremental-export performance claim.
The live replay uses incremental state export and waits for actual drawing acknowledgements.

The TLS replay counter includes both directions, connection setup, WebSocket framing, applied acknowledgements, and simulated immediate presented acknowledgements.
It excludes browser assets, TCP/IP headers, authorization-service traffic, rendering delay, and input control.
The browser runtime asset cost is the same as in the [broader benchmark](structured-bandwidth.md).
Video rates include encoded H.264 payload only.
The video sweep uses the same veryfast/zerolatency settings and exact source pixels as the broader benchmark.
These boundaries favor video on transport overhead but do not establish equal startup cost, recovery behavior, or perceptual quality.

## Live replay

The [runner](../demos/structured/live.html) streams the generated RUI packets over WebSocket at up to 25 updates per second.
The browser returns a presented acknowledgement after drawing each update.
The source waits for that acknowledgement before the next update, so a slow client cannot create an unbounded queue.
Pause withholds presentation readiness; local zoom redraws retained state without requesting more host geometry.
The final scene remains available for local zoom after the replay ends.
The [live check](../demos/structured/check-live.mjs) verifies both representations, pause/backpressure, zoom without traffic, received byte totals, and the final frame against the offline reference.

The server is a read-only trace fixture, not a replacement for the authenticated producer server.
It checks the browser origin, bounds incoming frames, and exposes only the two generated replay streams and runtime assets.
The client has no remote control grant.
Use the [run instructions](../demos/structured/README.md#source-aware-object-and-camera-experiment) to generate traces and open the demo from the desktop or a phone over Tailscale.
The included narrow-layout image is a desktop browser check, not a physical-phone performance result.

## What the experiment establishes

Preserving source structure can reduce traffic beyond compressing the final drawing operations.
The current protocol already has useful building blocks: retained geometry, parent transforms, stable identities, and atomic publications.
The adapter can obtain a large part of the benefit by observing objects and camera changes before they become world-space beams.
The result does not require game-specific client physics or trust in executable content sent by the source.

It does not prove that grouping arbitrary output will find the original objects, that all applications expose such structure, or that more client rendering work is always worthwhile.
The captures are short and deterministic; source capture/encoding CPU, physical-phone energy, real input latency, and lossy Internet conditions remain separate measurements.
A 3D camera/mesh profile would need its own projection, resource, clipping, and renderer contracts.

## Possible next directions

**3. Hybrid media as an explicit profile.**
Keep semantic controls and useful scene state while sending video or raster regions where those are cheaper.
Start with embedded video passthrough or a bounded dynamic region, with explicit timing, recovery, and representation changes.
Test unseen mixed workloads and count both modes' startup costs before adding automatic selection.
The sparse editor's dirty-PNG result is a reason to include dirty raster alongside H.264 in that selection experiment.

**4. Further transport and representation tuning.**
First suppress unchanged capture publications and reduce repeated publication metadata, since small updates now pay a substantial fixed cost.
Then test field-level transform patches, exact numeric prediction, and an opt-in screen-space precision budget for dense changing geometry.
Retain lossless fallback and verify quality at the maximum supported local zoom.
The chart favors lower-quality H.264, and Asteroids remains above the earlier proposed 0.13 Mb/s target, so these are concrete tuning cases.

**Additional validation and renderer work.**
Measure real applications, physical phones, cold and warm HTTP caches, and key-to-photon latency under bandwidth limits and loss.
Profile full-state export, many small drawing primitives, coverage-mask caching, and native video decoding on those devices.
The broader text fixtures show why bandwidth, rendering cost, and battery energy must be evaluated separately.
A cooperating application integration and shared shape resources are useful follow-ups before attempting to infer objects from MAME's flat beam stream.

## Evidence

[Retained results](../demos/structured/evidence/source-aware/summary.json) include source hashes, exact state checks, packet and TLS counts, recovery, renderer samples, codec settings, and pixel hashes.
The fixture directory also retains the live browser check and screenshots.
`demos/structured/run-objects.py` reproduces the comparison; `demos/structured/check-live.mjs` checks the live replay.
