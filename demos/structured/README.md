# Structured workload benchmark

This harness measures authored editor, scrolling-text, chart, and diagram workloads.
It does not capture existing editor or diagram applications.
Each primary trace lasts 12 seconds at 25 samples per second, with the final quarter idle.
Text uses an authored 5×7 glyph alphabet carried as segment geometry, plus semantic text.
Scrolling retains the document and changes a parent transform.
The chart changes its data points; the diagram retains node shapes and changes their transforms and edges.

## Run

Build matching browser assets and exporters from the repository root.
Install the Python dependencies from `demos/games/bench/requirements.txt` in a virtual environment.
The benchmark also needs FFmpeg with libx264, OpenSSL, and Chromium.

```sh
cargo build --release --locked -p rui-browser-host --bin scene-trace -p rui-transport --example compression_probe
python3 client-wasm/build.py
npm --prefix client-browser run build
python demos/structured/run.py --output /tmp/rui-structured
python3 -m unittest discover -s demos/structured -p 'test_*.py' -v
node demos/structured/check-phone.mjs /tmp/rui-structured
```

Use a new output directory for an independent run.
`--seconds` changes the logical trace duration.
A longer state/transport run can use `--seconds 60 --skip-render --skip-delayed`; this does not make a paired 60-second video comparison.
`--skip-render` omits pixel and codec runs; `--skip-network` omits live TLS fixtures.
The primary comparison uses 320 × 240 and 640 × 480.
Reference rendering uses full-state export for compatibility with the earlier game benchmark.
The portable browser runner uses the same incremental export method as BrowserClient.
Each reference rendering size has one untimed export run and one timing run, so CPU results are exploratory rather than repeated device performance gates.
The video sweep uses veryfast, zerolatency, no B-frames, one-second keyframes, and one encoding thread.
It includes CRF18, CRF28, lossless RGB H.264, and an exact changed-bounding-rectangle PNG baseline.
Lossless H.264 must reconstruct every rendered pixel exactly.

`scene-trace` uses the production publication builder and verifies each incremental state against its full snapshot.
Recovery exports start from a fresh snapshot halfway through the same timeline.
Both compressed streams must match direct core replay in the built WASM receiver.
The network fixture opens a new TLS connection for each stream and counts both directions, including TLS setup, WebSocket framing, generated applied acknowledgements, and simulated presented acknowledgements.
The wire fixture simulates immediate presentation readiness; real rendering is measured separately.
Its certificate is private to an ephemeral loopback fixture and is verified by its client.
It is a replay endpoint, not a production authorization or input server.
The 20 ms setting delays each message delivery; it does not emulate packet loss, congestion, or a real Internet connection.
Acknowledgement latency excludes drawing and input dispatch.
TCP/IP headers, browser assets, and authorization-service traffic are outside the TLS byte counter.
The summary reports browser asset sizes separately.

Every sampled source capture produces a publication, including unchanged idle samples.
This is conservative for applications whose adapters suppress unchanged captures.
H.264 receives the same idle frames and can encode them cheaply; dirty PNG sends no unchanged frames.
Do not extrapolate these fixtures to all UI applications, fonts, or encoder settings.

## Browser and phone replay

```sh
python3 demos/structured/serve.py --data /tmp/rui-structured --host 0.0.0.0 --port 8099
```

Open the machine's Tailscale address at port 8099 on the phone.
Select a workload and canvas resolution, then select **Run**.
Select **Save result** after playback to export device metadata, decode/export/render timing, memory accounting, and trace bytes.
The server exposes only browser runtime files, this page, and workload publication packet files from the selected evidence directory.
This runner downloads a trace and replays it locally; it is not a live key-to-photon or network-latency measurement.
Repeat after clearing site data for cold assets, and without clearing for a warm run.
Battery energy needs separate device instrumentation.
`check-phone.mjs` verifies the touch layout in desktop Chromium; its result explicitly says that no physical phone was measured.

## Source-aware object and camera experiment

Generate and measure an authored 32-object scene in retained and flattened forms.
The retained form changes eight object poses and one camera transform, while object geometry stays cached.
The flattened form projects the same endpoints into one changing segment batch.
The script requires identical pixel hashes at 320 × 240, 640 × 480, and 1280 × 960 before it compares either form with video.
The flattened renderer sends per-frame SHA-256 hashes to the harness, so only the retained-object movie needs raw pixel storage.
Use `--reuse-render` after an interrupted run; complete captures are reused only when publication, renderer, and WASM hashes match and the required pixel data is present.

```sh
python demos/structured/run-objects.py --output /tmp/rui-objects
node demos/structured/check-live.mjs /tmp/rui-objects
python3 demos/structured/serve.py --data /tmp/rui-objects --host 0.0.0.0 --port 8099
```

Open `http://HOST:8099/demos/structured/live.html`, where `HOST` is the desktop or its Tailscale address.
Select a representation and select **Play from host**.
Pause, change local resolution, then resume.
The server waits for real browser presentation acknowledgements and keeps at most one publication in flight.
A pause longer than 60 seconds can close the fixture connection; select **Play from host** to restart.
The page retains the final image so that resolution can also change after playback.
Its byte counters count received RUI packets, excluding network framing.

This is a live WebSocket replay of host-authored state, not a live emulator or client-side simulation.
The trace server is a bounded read-only fixture with no production authorization or input control.
The authenticated MAME demo remains separate.
The [source-aware report](../../docs/source-aware-demo.md) records the measured benefit and limits.
