# RUI

RUI is an experimental remote UI protocol and Linux host for using applications from a web browser, including a phone.
The host owns application state; clients receive authorized UI semantics and supported vector scenes, with one remote controller at a time.

Its [design foundations](RFC.md#2-foundations-and-alternatives) include GTK4/GSK's retained rendering, AccessKit's semantic updates, Remote DOM's UI replication, and Qt Remote Objects' source/replica model.
RUI combines visual and semantic replication with authentication, control leases, guarded commands, and reconnect recovery.
It sends UI state instead of video frames: a client can mirror supported source layouts or build responsive web controls from semantics.

Today, the [desktop workspace](apps/desktop/README.md) shares existing Linux apps through accessibility, and the [browser workspace](apps/browser/README.md) controls a dedicated host Chromium.
Cooperating GTK demos also support vector mirroring.
Coverage depends on the source; existing-app accessibility is partial, and the strict vector profile excludes raster content.
The direction is a useful mobile interface to an existing Linux session, with broader app coverage, richer browser presentation, and better performance; see [status and next steps](docs/status.md).

## Try it

On a graphical Linux desktop, install the [prerequisites and WASM tooling](docs/getting-started.md#prerequisites), then run from the repository root:

```sh
python3 apps/desktop/run.py --http-local
```

In another terminal, choose an already-running app to share:

```sh
python3 apps/desktop/share.py
```

Open the printed invitation URL in a separate client browser, compare its pairing code, and type `approve CODE` in the host terminal.
Choose **Take control** to use the shared app.
Stop with `quit` or Ctrl+C; attached apps keep running.
Later runs can add `--skip-build`.

For a phone connection, follow the [desktop HTTPS/Tailscale setup](apps/desktop/README.md#run).
Add `--with-browser` for a managed host browser, or try the [standalone browser workspace](apps/browser/README.md#start-locally).
To compare responsive semantic controls with vector mirroring, follow the [GTK demo walkthroughs](docs/getting-started.md#cooperating-gtk-demos).

## Documentation

- [Documentation index](docs/README.md): guides, components, protocol profiles, and reports.
- [Development and verification](docs/development.md): builds, examples, and live checks.
- [Status and next steps](docs/status.md): supported scope, limitations, and direction.
- [Protocol design](RFC.md): original rationale and architecture.
