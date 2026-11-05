# Vector arcade

Run the original Asteroids in native MAME and control it through the RUI browser client.
The host owns emulation and exports MAME's vector-device line callbacks before rasterization.
RUI carries ordinary semantic controls and retained vector scene transactions over authenticated TLS/WebSocket transport.
The browser uses the existing Rust/WASM replica and CanvasRenderer.
No screenshot, video, game executable, or game simulation is sent to the browser.

## Run

Install MAME 0.289 and the [RUI build prerequisites](../../docs/getting-started.md#prerequisites).
Supply a MAME-compatible `asteroid.zip` in a local ROM directory.
Game data is not included in this repository.
Run on a graphical Linux desktop:

```sh
python3 demos/games/run.py --rompath /path/to/roms
```

Open the printed browser URL and choose the printed `client.credential` file.
Choose **Connect**, then **Take control**.
Press **Insert coin**, then **Start**.
Use the on-screen hold buttons or the keyboard controls below the canvas.
Click the canvas to return keyboard focus to the game.
The render-scale control changes local vector rasterization without requesting a new source resolution.
The page reports presented frames, received RUI bytes, draw time, and retained line slots.
Byte counts include RUI messages and exclude TLS and socket overhead.

Use `--skip-build` on later runs.
Use `--mame /path/to/binary` for a custom MAME executable.
Use `--plugins /path/to/mame/plugins` for an extracted MAME distribution.
Use `--state /path/to/new-directory` to retain logs and the temporary credential after shutdown.
Use `--public-host HOST --cert CERT --key KEY` for a browser-trusted HTTPS connection from another device.
Copy only the client credential to that device through your existing secure channel.
The backend binds loopback; Vite verifies the backend certificate and checks the browser Origin.
One credential and one remote controller are supplied by this development demo.

## Boundaries

The adapter presents fixed-width opaque vector beams on black.
MAME's additive blending, CRT glow, cabinet artwork, and audio are excluded.
This presentation is a declared vector approximation of the native game's appearance.
Every beam becomes an ordinary transformed RUI rectangle.
Line slots remain allocated when a beam disappears, so continuous play does not retire and reissue scene identities each frame.
The source has a 2,048-line ceiling and sends the latest frame at up to 30 publications per second.
The host coalesces frames while the client acknowledges its preceding presentation.
This demonstrates vector transport, rather than game-object identities or cached asteroid meshes.

The optional `game.vector.v1` profile permits terminal `Dispatched` results for held semantic controls.
The source waits for MAME to acknowledge the exact input revision and mask before returning that result.
This acknowledges native input dispatch; it does not assert a gameplay effect or causal visual presentation.
The existing command ledger, control lease, target clocks, and presentation readiness remain active.
The browser coalesces desired held states, sends releases first, and refreshes active inputs.
Focus loss releases control, and lease cleanup clears the native inputs.
A worker watchdog clears held input after 350 ms without a command.
A separate MAME-side expiry clears inputs if the worker disappears, within three wall-clock seconds.
Pausing MAME also pauses its input callbacks; they check expiry when emulation resumes.

## Checks

```sh
cargo test --locked -p rui-browser-host -p rui-client-core
npm --prefix demos/games test
npm --prefix demos/games run build
RUI_MAME=/path/to/mame RUI_ROMPATH=/path/to/roms RUI_MAME_PLUGINS=/path/to/mame/plugins python3 demos/games/check.py --xvfb /path/to/Xvfb
```

The live check owns authenticated Xvfb displays and a Chromium instance.
It verifies vector frames, native coin/start/held input, multiple keys, local zoom, and release after blur and disconnect.
It also measures presentation rate and native input acknowledgement latency.
Set `RUI_GAME_REPORT` and `RUI_GAME_SCREENSHOT` to retain the JSON result and browser image.
