# Directions for reducing RUI bandwidth

## Executive summary

**RUI has a credible path to lower bandwidth than H.264 on structured graphics.**
New offline probes put the existing game publications near the tested H.264 bitrate with bounded temporal compression alone.
A compact representation of the captured beams uses **0.122 Mb/s for Battlezone and 0.095 Mb/s for Asteroids**, versus **0.323 and 0.217 Mb/s** for H.264 CRF18 veryfast at 640 × 480.
The compact representation preserves every captured floating-point value and rebuilds byte-identical RUI publications, but its sizes exclude protocol records and it is not a deployed client path.

**Start with transport compression, then preserve geometry before it becomes many scene records.**
For larger gains, transmit reusable shapes, object motion, camera changes, animation rules, and semantic edits.
Move work to the client where the data saved justifies its processing and startup cost.
Keep a common authority model while allowing different visual representations.

**Treat universal superiority as a portfolio goal, not a promise that vectors always win.**
Use retained semantics and geometry where they are compact, and media or raster regions where they are cheaper.
An existing H.264 video can already be sent as its encoded bytes; extra semantic messages do not guarantee a smaller result.
The practical target is large wins on structured workloads and near-video cost on workloads with little reusable structure, at comparable quality and latency.

## What the current bytes represent

The [paired comparison](game-vector-raster-comparison.md) measures a particular adapter, not the minimum information needed to reproduce the games.
The [game projection](../browser-host/src/projection.rs) converts each beam into a rectangle, a six-component transform, a color, and a scene record.
Coordinates, colors, and transforms use 64-bit floating-point fields in [the schema](../spec/rui.proto).
The [publication builder](../adapters/sdk/src/publication_builder.rs) sends a complete changed record.
It already avoids unchanged records and normalizes versions; adding generic tree diffing would repeat work that exists.

The source exposes a sequence of beams, without asteroid or tank identities.
Array slots become scene identities, so insertion or reordering can make many unrelated records appear changed.
The projection also computes lengths and normalized directions that a receiver could derive from endpoints.
A byte compressor cannot directly exploit all of these mathematical relationships.
Scene mutations account for 97.1% of warm Battlezone publication bytes and 95.7% of Asteroids bytes.
Envelope cleanup alone therefore cannot solve this workload.

There are three distinct opportunities:

| Opportunity | What changes | Example |
| --- | --- | --- |
| Encode the same state better | Transport or wire representation | Compress repeated fields and previous publications |
| Preserve a better description | Adapter and scene vocabulary | Send endpoints or a cached mesh with a transform |
| Reproduce behavior locally | Execution boundary | Send an animation rule or simulation input instead of each result |

The last two can remove information that today's transport layer must otherwise encode.
They also require a stronger contract between the source and receiver.

## New measurements on the retained game traces

These probes reuse all 200 frames per game from the [retained comparison](../demos/games/evidence/comparison/README.md), covering eight seconds at 25 Hz.
The H.264 baseline remains the same paired, zero-B-frame, `zerolatency`, one-second-keyframe encoding.
It is one useful operating point, not an optimal H.264 rate–quality curve.
All rates below are payload Mb/s, excluding network framing and control traffic.

### Compression of complete current publications

| Representation | Battlezone | Asteroids | Change required |
| --- | ---: | ---: | --- |
| Current RUI | 3.714 | 1.693 | None |
| Independent zlib level 6 | 0.687 | 0.428 | Compression negotiation |
| zlib level 6, history reset each second | 0.577 | 0.267 | Compression history |
| Independent Zstandard level 3 | 0.687 | 0.431 | Compression negotiation |
| Zstandard level 3, history reset each second | 0.336 | 0.220 | Compression history |
| Zstandard level 6, history reset each second | 0.317 | 0.201 | More encoder work |
| Zstandard level 3, continuous bounded history | 0.303 | 0.192 | Longer history lifetime |
| H.264 CRF18 veryfast at 640 × 480 | 0.323 | 0.217 | Raster reference |

All compressed RUI variants reconstruct the exact complete core envelopes.
Zstandard uses a maximum 256 KiB back-reference window; this is not a bound on total codec memory.
Every publication is flushed and decoded before the next arrives.
No variant waits for a second of frames or examines future frames.
The continuous case retains bounded history for the eight-second clip.
A history reset starts a new compressed stream; it does **not** create a scene checkpoint from which a new client can recover.
The current-publication probes include only the initial scene snapshot, while the video reference has a keyframe each second.
Match checkpoint and recovery requirements, or test longer video keyframe intervals, before claiming a deployed bandwidth advantage.

At level 3 with one-second resets, RUI is within about 4% of native-resolution H.264 in both games.
Level 6 falls below that H.264 payload by about 2% and 7%, respectively.
That margin is too small to claim a complete network win without measuring framing, recovery, and session traffic.
The continuous level 3 case is about 6% and 12% smaller, but has different recovery behavior from the video reference.

For level 3 with one-second resets, compression-call p95 was 0.062 ms for Battlezone and 0.036 ms for Asteroids; decompression-call p95 was 0.025 and 0.011 ms.
These are Python calls into native codecs on the development desktop, across three runs including context setup.
They exclude browser/WASM costs, total memory, network transfer, and rendering.
Compression preserves today's client rendering cost, which the earlier benchmark found much higher than video decoding.

### Compact source geometry

| Source representation with Zstandard level 3 | Battlezone | Asteroids |
| --- | ---: | ---: |
| Ordered arrays of seven float64 values per beam | 0.122 | 0.095 |
| Exact decimal integers with differences from the previous slot | 0.155 | 0.052 |
| Float64 arrays with byte shuffling | 0.443 | 0.362 |
| H.264 CRF18 at 320 × 240 | 0.155 | 0.120 |
| H.264 CRF18 at 640 × 480 | 0.323 | 0.217 |
| H.264 CRF18 at 1280 × 960 | 0.668 | 0.439 |

Each source packet contains a line count and ordered endpoint/color data.
Compression history resets every 25 frames and flushes after every frame.
The integer predictor also resets at these boundaries.
All variants reconstruct the exact IEEE floating-point bits in the trace.
Re-export through the actual game projector and publication builder produces byte-identical original RUI envelopes for all three variants.
This proves source fidelity; an eventual new browser primitive must still pass rendering conformance.

The float64 result is about **62% smaller than native-resolution H.264 for Battlezone and 56% smaller for Asteroids**.
It omits the RUI session, surface, scene, semantic, version, and command metadata, so it is a representation estimate rather than a replacement protocol benchmark.
For scale, an assumed 16 extra bytes per update at 25 Hz adds 0.0032 Mb/s; actual metadata and recovery costs must be counted.
The result gives enough margin to justify a compact beam prototype without first extracting game objects.

The decimal representation exploits precision already limited by this capture: four decimal places for endpoints and six for colors.
It rejects inputs that do not round-trip exactly; applying the same precision to arbitrary source geometry would be lossy.
It helps Asteroids more than Battlezone, while simple float64 arrays win for Battlezone.
Byte shuffling loses on both traces despite its usefulness in scientific compression, as illustrated by [Blosc's typed-data filters](https://blosc.org/pages/blosc-in-depth/).
Preprocessing must be measured on the actual stream.
The reductions in these tables are alternatives or combined experiments, not factors that can be multiplied together.

## Transport and wire changes

### Bounded compression with useful history

Add a negotiated compression layer outside the core reducer, preserving the exact decoded publication and existing validation.
Start with Zstandard at modest levels and compare continuous history, periodic resets, and independent messages with a shared dictionary.
The new measurements favor history reuse much more than changing the independent-message codec.
[RFC 8878](https://www.rfc-editor.org/rfc/rfc8878) defines Zstandard dictionaries; their distribution and identity still need an RUI contract.
Train dictionaries on separate earlier sessions, charge their delivery bytes, and test unseen workloads.
A predefined dictionary can improve small independent messages without making them depend on every preceding message.

WebSocket `permessage-deflate` is another route, with [standardized context takeover](https://www.rfc-editor.org/rfc/rfc7692).
Today's [transport](../transport/README.md) deliberately does not select compression, so this needs negotiated behavior and tests, not a proxy setting alone.
The zlib probes are not exact WebSocket packet measurements.
An explicit application layer offers clearer control over history lifetime, decoded-size budgets, and browser/native parity; WebSocket compression may reduce integration work where supported.
Compare both before choosing.

Keep histories within one authorized view, reset them at policy or epoch changes, and bound decoded output before allocation.
Compression dictionaries that mix secrets with attacker-controlled content can expose information through lengths; choose history boundaries accordingly.
Account for dictionary memory, compressor tables, staging buffers, and the decoder download on a cold browser.
Do not compress already encoded media again unless measurement shows a gain.

### Typed patches and predictable data

| Direction | Mechanism | Best use and constraint |
| --- | --- | --- |
| Field patches | Send object ID, field mask, and changed values | Transform-only updates; preserve version and base checks |
| Shared styles | Intern color, stroke, font, clip, and common transforms | Repeated UI or beams; count style-table updates |
| Small local handles | Map long resource hashes and object identities to compact session handles | Repeated references; retain canonical identity at trust boundaries |
| Typed arrays | Batch coordinates, colors, and indices; omit per-element record tags | Dense paths and charts; preserve paint order |
| Numeric predictors | Encode endpoint differences, repeated values, or XOR differences in float bits | Structured numbers; choose predictors by measured residual size |
| Binary deltas | Copy ranges from a known previous publication and encode the rest | A generic bridge; include base identity and recovery cost |

File distribution already uses source-based delta formats such as [VCDIFF](https://www.rfc-editor.org/rfc/rfc3284.html).
An RUI version could encode a target publication against the receiver's confirmed base while leaving the core reducer unchanged.
It may add little after temporal Zstandard; test the combination instead of assuming another large gain.
Numeric arrays also allow geometry-specific codecs, as explored by [Khronos mesh compression](https://github.com/KhronosGroup/glTF/blob/main/extensions/2.0/Khronos/KHR_meshopt_compression/README.md), although that linked extension is a release candidate and need not become an RUI dependency.

Stabilize identity and serialization before adding sophisticated deltas.
Preserve source object IDs, separate style from geometry, and avoid rebuilding child lists when only positions change.
Canonicalize equivalent encodings only where ordering has no visual meaning; sorting overlapping translucent primitives can change the image.
Transport identities, semantic target identities, and compact visual handles need not be the same identifiers.

### Coalescing and separate delivery rules

The traces contain 87 Battlezone and 35 Asteroids samples whose beams exactly match the previous sample.
Suppress repeated visual payloads and separate visual changes from capture provenance or command acknowledgements where the authority contract permits it.
Video also encodes repeated frames cheaply, so this is useful housekeeping rather than an exclusive advantage.
Avoid accumulating stale states behind a slow receiver; build an update from a valid base to the newest useful state.

Separate reliable semantic changes and resource delivery from rapidly superseded visual state.
The current producer's wait for presentation readiness can limit throughput to one publication per round trip.
Pipeline visual delivery with bounded queues while retaining the presented revision needed for input admission.
Never drop an arbitrary transaction from a dependent delta chain.
An independently applicable state update or an update against an acknowledged base can support selective discard.

[QUIC streams](https://www.rfc-editor.org/rfc/rfc9000.html) can avoid loss on one stream blocking unrelated streams.
They do not reduce the information content of scene updates, and a compressed stream still depends on its earlier bytes.
Treat WebTransport or datagrams as scheduling and loss-recovery choices, with independent blocks or explicit bases where needed.

## Representations that retain more structure

### Compact paths and reusable objects

A batch of endpoints is the most direct next step for these games.
Give it a defined width, cap rule, color encoding, draw order, and behavior for zero-length beams.
The current rectangle lowering uses a minimum length and fixed width; a generic line API will not automatically reproduce its pixels.
Avoid creating a separately managed scene object for every beam unless that object needs independent interaction or reuse.

RUI already has path resources and transformed scene nodes.
Reuse a static path for an asteroid, icon, glyph, or chart marker and send only its instances and changing transforms.
Batch paths with matching style when this preserves draw order.
For frequently changing paths, compare a compact inline primitive or a versioned buffer update against creating a new immutable resource every frame.
Do not silently mutate a resource behind its existing content hash.

At the source, capture the scene before reusable objects are flattened into beams or pixels.
For Asteroids this could expose shape, position, angle, and visibility.
For Battlezone it could expose static world geometry, tank poses, and a camera, with projection performed locally.
The existing MAME beam hook does not provide these objects; obtaining them requires a deeper adapter or inference.
The gains can be large because a camera change can replace updates to hundreds of projected endpoints.
Client geometry, clipping, precision, and rendering cost become part of the contract.

### Animation and state prediction

Transmit an animation's start state, clock, duration, easing, and end state once, then send changes or cancellation.
A blinking caret, scroll animation, spinner, orbit, or moving chart cursor does not need a fresh description each display frame.
The [Web Animations timing model](https://www.w3.org/TR/web-animations-1/) is a useful precedent for defining behavior over time.
Use a bounded declarative vocabulary before considering executable client programs.

For less predictable motion, send position and velocity, then correct the receiver when predicted error exceeds a limit.
This applies the state quantization, delta bases, and motion reconstruction used in [networked game snapshots](https://new.gafferongames.com/post/snapshot_compression/).
Interpolation adds delay; extrapolation can be wrong.
Specify error, correction behavior, clock drift, and recovery at collisions or other discontinuities.
Keep authoritative interaction state distinct from predicted presentation.

An illustrative budget is 20 changed objects × 24 bytes × 20 updates/s = **0.0768 Mb/s**, before assets, headers, and corrections.
This is a sizing example, not a measurement of either game.
Predictable periods could require fewer updates; unstable or dense scenes could require more.

### Replicate the application state or its inputs

A semantic document, table, chart, or editor can transmit data and edits while the client performs layout and drawing.
This can make network cost depend on changed content rather than the number of affected pixels.
RUI's existing semantic and resource layers provide a starting point.
Define whether the task requires identical pixels or equivalent information and interaction; responsive native controls may satisfy the latter without satisfying the former.
For exact text appearance, retain host shaping or versioned font/outline resources rather than assuming every device shapes text identically.

At the far end, send assets, a deterministic program, initial state, and ordered inputs.
[Deterministic lockstep](https://new.gafferongames.com/post/deterministic_lockstep/) makes steady-state traffic depend largely on inputs, but requires matching simulation behavior and can stall on missing input.
Authoritative corrections or rollback can address some failures at additional complexity and cost.
For a game, this approaches a client-side port or replicated emulator, with startup downloads, device CPU, version compatibility, and resynchronization to count.
It offers a high bandwidth ceiling for cooperating applications but is a different product boundary from generic desktop remoting.

## Quality and visibility as explicit choices

H.264 CRF18 is lossy; an exact geometry protocol should also be compared against exact video, while optional lossy geometry should be compared at matched visible error.
Keep an exact profile and add negotiated visual approximation where the task allows it.

| Direction | Information avoided | Condition |
| --- | --- | --- |
| View-dependent precision | Coordinate bits that cannot affect the current view | Recompute precision for zoom and device scale |
| Level of detail | Subpixel segments and excessive curve samples | Preserve thin lines, topology, and interaction targets |
| Temporal error bounds | Tiny motion and color changes below a threshold | Limit cumulative drift and visible stepping |
| Interest and occlusion filtering | Invisible or irrelevant geometry | Prefetch likely reveals; preserve required semantic state |
| Progressive refinement | Immediate transmission of all detail | Send a fast usable state, then refine without delaying controls |

For nearest rounding with coordinate step `q` and display scale `s`, each endpoint coordinate differs by at most `s × q / 2` screen pixels under uniform scaling.
That does not bound antialiasing, stroke coverage, or errors from later transforms.
Validate rendered pixels and thin-line stability, not only coordinate error.
Avoid reducing precision of text content, action targets, or authoritative state merely because the visual layer permits approximation.

Rate selection should consider bytes, latency, CPU, memory, and energy together.
A smaller geometry stream that makes a phone rasterize thousands of paths may lose to hardware video decoding on battery use.
GPU batches, retained coverage, and damage-based rendering are supporting work even when they do not directly reduce bytes.
Smaller network payloads alone do not remove the current high-resolution renderer limit.

## A broader architecture for mixed content

Choose a representation per coherent region or layer: semantic layout, retained vector objects, compact dynamic geometry, raster tiles, or video.
Use unchanged-region reuse and copy operations for scrolls where appropriate.
An embedded video should ideally retain its existing encoded stream, with local composition of controls and text, rather than pass through decode, screen capture, and re-encode.
Source access, synchronization, clipping, and supported media formats determine when this is possible.
For example, an editor could retain exact local text while only its embedded video pane uses video delivery.
Source motion, camera, and region information could also guide a video encoder; test that option when comparing source-aware systems.

This direction has precedents: [SPICE combines display operations, caches, and video streams](https://www.spice-space.org/spice-protocol.html), while [RDP negotiates multiple graphics codecs including H.264](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpegfx/31c6e2b1-335b-4a75-9454-bb2309958c21).
RUI's differentiator would be its shared semantic state and authority across visual modes.
The current strict profile excludes raster content, so a hybrid path needs a new negotiated profile.
It can preserve the existing exact vector profile for applications that require it.

Use one authoritative scene and semantic revision model, with separate encoders for its visual regions.
The receiver composes those regions against explicit scene versions and timestamps.
Mode changes must replace the old representation atomically, so stale video cannot cover a newer control or remain after a surface is removed.
Allow video-only content to incur minimal semantic overhead when the user does not need a semantic projection.

A selector should minimize estimated transmission and client cost subject to quality and deadline constraints.
Include startup assets, codec switches, keyframes, cache misses, and error corrections in that estimate.
Use occasional probes or past measurements, with a minimum dwell time to avoid frequent switches.
Encoding every region in every format on every frame can cost more CPU than the traffic saved.
Independent tiles also lose some cross-region video prediction, so fine-grained hybrid delivery does not always win.

For scale, a hypothetical 250 kB extra asset download with a 0.1 Mb/s state stream beats a 0.3 Mb/s video stream only after **10 seconds**, ignoring shared overhead.
The crossover follows `8 × extra_asset_bytes / (video_bits_per_second − state_bits_per_second)` when the denominator is positive.
Report cold and warm sessions separately, including cache eviction and reconnects.

## Further research directions

These are proposals for RUI, not claims of new inventions or measured gains.

| Proposal | Experiment | Why it could help | Main uncertainty |
| --- | --- | --- | --- |
| Infer objects from flat graphics | Group beam chains and fit shared translations, rotations, or camera motion; encode exact residuals | Recover structure from applications without object APIs | Grouping cost and unstable correspondence |
| Select the shortest explanation | Compare literal arrays, reusable templates, transforms, temporal models, and residuals within each region | Use whichever description is smallest after model cost | Online search and avoiding excessive encoder work |
| Predict rendered output | Render a cheap local approximation and send a sparse image or geometry correction | Cover effects that are expensive to describe exactly | Matching the predictor and efficiently coding its residual |
| Learn a session dictionary | Learn recurring paths, subtrees, or update patterns from past states only | Exploit an application's repeated structures | Startup cost, cache pressure, and generalization |
| Describe procedural effects | Send a seed, bounded generator, parameters, and time range for suitable effects | Replace large predictable particle or texture sequences | Determinism and client execution budget |
| Encode changes by cause | Send a scroll offset, camera pose, style change, or layout constraint that explains many outputs | Avoid thousands of consequences of one change | Capturing the cause before flattening |

Local prediction plus a transmitted residual has a direct research precedent in [Kahawai's collaborative rendering](https://www.microsoft.com/en-us/research/publication/kahawai-high-quality-mobile-gaming-using-gpu-offload/).
For RUI, a useful first version could retain exact text and geometry locally while sending residual effects in a separate profile.
Do not assume its published gains transfer to these workloads.

Learned entropy models are another long-term option: predict the distribution of scene updates from previous state, then entropy-code the actual updates.
[Conditional video entropy coding](https://arxiv.org/abs/2008.09180) supplies a related research direction.
A deterministic decoder with exact residuals can preserve content; an image generator that invents details cannot serve as an exact UI replica.
Model downloads, inference cost, and cross-device reproducibility make simple dictionaries and explicit predictors better first experiments.

Rendering systems already distinguish display lists, transform/scroll properties, and composited layers, as described in [Chromium's rendering architecture](https://developer.chrome.com/docs/chromium/renderingng-architecture).
Capturing these structures is likely more useful than trying to reconstruct them from final pixels.
For many applications, the key architectural decision is where to observe the computation, not which final byte compressor to select.

## Where RUI is most likely to beat video

| Task | Most promising representation | Expected boundary |
| --- | --- | --- |
| Forms, editors, terminals, tables | Semantic edits, cached glyphs, local layout | Large initial documents and exact font fidelity |
| Whiteboards, diagrams, plots, maps | Paths, reusable symbols, incremental data, local pan/zoom | Dense changing imagery and large asset downloads |
| Asteroids and similar vector games | Compact beams first; objects and motion later | Current source exposes no game object model |
| Battlezone and CAD views | Cached geometry, camera and object poses | Source access, clipping, and client rendering capacity |
| Predictable animation | Timelines or bounded local simulation | Interruptions and unpredictable effects |
| Video and camera content | Encoded media passthrough or video regions | Little inherent semantic bandwidth advantage |
| Arbitrary graphics on weak clients | Adaptive video/raster path with semantic controls | Hardware decoding may dominate total efficiency |

A sparse UI update already costs 237 bytes in the existing [controlled performance fixture](performance.md), with cached resources reused.
That is evidence of a useful mechanism, not a paired H.264 victory for general UI workloads.
Measure representative tasks against video that also skips idle work and uses appropriate regions and quality settings.
H.264 is not an information-theoretic bound; a better raster codec can also beat it on some tasks.
The source-aware opportunity is to exploit structure that a pixel stream does not expose.
That advantage can disappear when the source supplies only unpredictable imagery or already compressed media.
Hybrid delivery can approach the best available mode, but finite-session overhead and imperfect selection prevent a universal dominance guarantee.

## Recommended sequence and decision gates

1. **Prototype bounded Zstandard on current publications.**
   Keep the core semantics unchanged and compare levels 1, 3, and 6, one-second resets, continuous history, and shared dictionaries.
   Gate on real browser decode, complete session bytes, bounded memory, reconnects, and controller latency.
2. **Implement a compact beam or dynamic-path profile.**
   Preserve order and endpoint fidelity, compare float arrays with exact integer predictors, and verify the current width and degenerate-beam rules.
   Target at most 0.20 Mb/s Battlezone and 0.13 Mb/s Asteroids on these fixtures with protocol metadata included; these are proposed payload gates, not measured production rates.
3. **Prove a broader structured-workload win.**
   Add paired editor, scrolling text, chart, and diagram tasks, including cold caches and long sessions.
   Test cached geometry, field patches, stable identity, and local animations before larger source integrations.
4. **Choose one source-aware architecture experiment.**
   Prefer reusable Asteroids objects or a small cooperating 3D scene with a camera before replicating a whole emulator.
   Include assets, corrections, startup, device CPU, and the value of local zoom in its result.
5. **Add hybrid media only as an explicit profile.**
   Use it to cover video and graphics that do not compress well as state, with synchronized semantic controls and atomic mode changes.
   Evaluate representation selection on unseen mixed sessions.

Use longer captures from multiple sessions, resolutions from phone to high-DPI desktop, and physical phones.
Compare exact RUI with lossless raster, and approximate modes with an H.264 quality sweep using matched foreground/text quality, frame rate, and latency.
Also include dirty-rectangle raster and modern codecs so a weak baseline does not drive architecture.
Count both directions, setup assets, dictionaries, snapshots, recovery, TLS/transport framing, and steady-state bytes.
Measure key-to-photon latency, CPU, memory, and phone energy under RTT, loss, and bandwidth limits.
Keep a training/evaluation split for dictionaries or inferred templates and record model costs.

The highest-value next implementation is **bounded compression followed by compact geometry**.
The largest architectural opportunity is **sending causes, reusable state, and behavior before they become repeated drawing operations**.

## Reproduce the new probes

Run from the repository root with Python 3.14 or newer including `compression.zstd`.
The retained measurement uses Python 3.14.8, Zstandard 1.5.7, and zlib 1.3.2.

```sh
cargo build --release --locked -p rui-browser-host --bin game-trace
python3 demos/games/bench/strategies.py --output /tmp/rui-strategies --game-trace target/release/game-trace
python3 -m unittest discover -s demos/games/bench -p test_strategies.py -v
```

The output directory must be new.
Without `--game-trace`, the script still checks exact compression and source-float reconstruction but does not verify re-exported RUI bytes.
The [probe script](../demos/games/bench/strategies.py), [retained results](../demos/games/evidence/strategies/summary.json), and [generated tables](../demos/games/evidence/strategies/tables.md) provide source hashes, per-frame byte counts, codec settings, timings, and verification results.
No production transport or protocol behavior is changed by this report.
