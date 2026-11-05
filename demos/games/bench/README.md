# Paired vector/raster benchmark

This harness compares the current RUI game visual projection with raster encodings of exactly its rendered frames.
It does not replace the game demo's transport, controller, or security policy.
The main findings are in [the report](../../../docs/game-vector-raster-comparison.md).

## Prerequisites

Use Linux with FFmpeg containing `libx264`, `libx264rgb`, and PNG encoders, Node, Chromium, and the existing RUI Rust/WASM build tools.
Create an analysis environment and build the exporter and browser library:

```sh
python3 -m venv /tmp/rui-comparison-venv
/tmp/rui-comparison-venv/bin/python -m pip install -r demos/games/bench/requirements.txt
cargo build --release --locked -p rui-browser-host --bin game-trace
python3 client-wasm/build.py
npm --prefix client-browser ci
npm --prefix client-browser run build
```

## Replay the retained workloads

The retained fixtures contain sampled beam geometry and capture metadata, without ROMs or credentials.
The output directory must be new.
Allow about 3 GB of temporary raw frame space for the default clips.

```sh
/tmp/rui-comparison-venv/bin/python demos/games/bench/run.py --fixture demos/games/evidence/comparison --output /tmp/rui-comparison-new
```

The runner verifies every decoded lossless frame and retains the commands, timings, bitrates, quality metrics, PNG samples, and plots.
It removes raw RGBA files after analysis unless `--keep-raw` is set.
Change `--chromium` to test another local Chromium executable.
The browser server binds a temporary loopback port and does not use the running demo's port.

## Capture a new workload

Supply MAME-compatible game data for Battlezone and Asteroids and an Xvfb binary.
The capture uses owned authenticated displays and does not attach to the user's running game.

```sh
/tmp/rui-comparison-venv/bin/python demos/games/bench/run.py --mame /path/to/mame --rompath /path/to/roms --xvfb /path/to/Xvfb --output /tmp/rui-comparison-fresh
```

The default schedule inserts coins, starts the game, and samples eight seconds at 25 Hz after setup.
It repeats two-second drive/fire, two turning/fire patterns, and idle phases.
The actual geometry is retained and hashed, so all variants within a run use the same workload despite native scheduling differences between captures.
Use `--seconds 4` through `--seconds 30` to change the capture duration.
Longer clips need more raw frame space.

## Stages and boundaries

| Script | Work |
| --- | --- |
| `capture.py` | Capture the native MAME beam callbacks and scripted input workload |
| `game-trace` Rust binary | Use the existing game projection and publication builder; verify transaction/snapshot reducer agreement; export exact core envelopes |
| `render.mjs` | Replay with the Rust/WASM receiver and existing CanvasRenderer; export paired pixels; measure three warm receiver/export/draw runs at each resolution |
| `codecs.py` | Encode the paired pixels with PNG and H.264; repeat encodes three times; verify lossless pixels; measure full-frame and foreground error |
| `decode.mjs` | Measure three warm browser decode/blit runs using PNG image decoding and WebCodecs H.264 |
| `summarize.py`, `tables.py` | Generate summary JSON, report tables, and comparison/quality figures |

RUI byte counts cover core publication envelopes, excluding session chunk framing, acknowledgements, and controls.
Raster counts cover encoded image/video payloads, excluding their prospective transport and control channel.
Both are payload comparisons, not packet-capture wire totals.
The visual trace omits the seven semantic input controls, which are not required to paint the game.

Browser RUI timings use full `Receiver.state()` export for a standalone replay.
The live BrowserClient uses incremental state export, so this replay's export cost is not the live client's exact cost.
Drawing uses the existing renderer in both cases.
Browser raster timings include decoding and canvas submission after preloading the encoded data.
WebCodecs requests `prefer-software`; the actual decoder backend is not inspected.
Encode CPU costs use FFmpeg process user/system time divided by the clip's frame count.
Encode wall time and all three runs remain in the JSON.
These are aggregate encode costs, not per-frame encoder latency percentiles.

Each browser run excludes the first five frames from warm timing summaries.
Three fresh receiver/renderer or decoder runs are retained for each resolution.
The initial RUI snapshot and video keyframes remain included in byte counts.
H.264 uses zero B-frames, `zerolatency`, a one-second keyframe interval, and one codec thread.
Both `ultrafast` and `veryfast` presets are retained where specified in the report.
PNG uses compression level 3.
RGB lossless H.264 is an exactness reference, not an assumed phone-compatible codec.

The hypothetical RUI zlib figures compress each publication independently at level 6.
They do not represent a shipped RUI compression feature.
The serialization and RTT figures are labeled models, without a network emulator or physical phone measurement.

## Checks

```sh
/tmp/rui-comparison-venv/bin/python -m unittest discover -s demos/games/bench -p 'test_*.py'
cargo test --locked -p rui-browser-host
```
