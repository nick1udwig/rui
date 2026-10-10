# Cooperating GTK application API

This pinned adapter accepts explicitly registered, application-owned GTK 4.22.4
content. It does not attach to an arbitrary application or infer a native object
from labels. The [document editor](../../demos/editor/app.py) is a complete second
consumer with document-owned state and ordinary GTK callbacks; it imports no fixture.

Create a `CoherentSource` and `NativeRegistry(source, window)`. Register root/group
semantics with `source.register`, then use `Controls(source, registry, surface_key)`
for labels, buttons, GtkEntry text and stable list items/collections. Readers return
typed values, tri-state states, ordered children, relations, grouping, reading order
and explicit text/collection availability. Unknown is not false; redacted and
unavailable text have no content, selection or editing actions. `entry(private=True)`
reads no private content. Typed integers are signed 64-bit and numbers must be finite.
This public producer rejects sensitive registrations without a redaction policy.

```python
source.register('root', 1, lambda: {'label': 'My application'})
controls = Controls(source, registry, 'content')
controls.entry('document', entry, 'root', 'Document')
capture = CaptureWidget(source, content, 'content', 'root', output)
window.set_child(capture)
view = RegisteredApplication(app, source, registry, output)
view.surface('content', capture, window,
             parent_matches=lambda: window.get_child() == capture)
dispatcher = NativeDispatcher(view.integration('content'), whole=view,
                              watchdog_seconds=0)
app.connect('shutdown', lambda *_: dispatcher.close())
```

The trusted launcher must supply the host's inherited private socket, dirty
channel and sealed lifecycle fence. Launch through `GtkCommandHost::spawn`; the
Python socket is never a public control API. `NativeDispatcher` rechecks source
incarnation, registered widget lifetime, target/text/model/focus clocks, native
modality, lease deadline and atomic fence on the GTK thread before execution.
Native exceptions after execution leave the common ledger's outcome ambiguous.
Exact retries belong to the Rust ledger. Source callback readers must be bounded
and side-effect free. Call `registry.retire(key)` before replacing a widget;
registering the same key then creates fresh semantic/text/binding identities.

`RegisteredApplication` owns up to four declared surfaces, logical placement,
stacking and parent/modal relationships. Register a parent before its child and
supply predicates that read the actual native parent/window objects. It arranges
fresh snapshots through the existing bounded capture barrier. The same immutable
node appended for local submission supplies vectors; semantics, widget bounds,
topology and native focus are re-read at the source boundary. A changed boundary
restarts or rejects the whole candidate. This is source-owned content composition,
not observed compositor placement or decorations. Size/paint callbacks outside
registered controls must call `source.changed()`; animation/blinking are disabled.

The original corpus now supplies the same `GtkApplicationSource` interface and
shares `CoherentView` assembly. Its scripted business rules and fault hooks remain
in fixture modules. Dispatcher action selection uses registered native types,
including scrolling any registered ListBox inside a ScrolledWindow.

The private bounded artifact handoff remains: manifests, exact envelopes and
authorized vector bodies are still verified by Rust. New applications default to
`diagnostics=False`, omitting local screenshot draw/PNG/BGRA work. Enabling
diagnostics adds source comparisons without changing wire content or capture
checks. In-memory IPC has not replaced this handoff or been claimed faster.
The vector audit checks actual optional diagnostic ownership separately from
coherent source evidence. Existing limits and permanent barrier slots remain.

Run the editor with `rui-demo --application editor` (add `--host-only` for a
separate browser). [Retained generic command evidence](../../fixtures/gtk/editor-interaction.json)
includes button/text/list effects, exact retry, stale draft, redacted password,
three dialog cycles, modal denial, widget replacement/retirement and zero host
ownership after disposal. Targets are chosen from roles/actions and ownership,
not labels. Independent Python/Rust replay checks every public semantic prefix.
Native whole-view, source restart, focus/selection ABA and production HTTPS
checks preserve the original application. No additional native backend or
arbitrary style coverage is claimed.

```sh
cargo build -p rui-producer-sdk --features gtk-fixture-hooks --bin gtk_command_check --locked
python3 fixtures/gtk/native_check.py --case editor --xvfb /path/to/Xvfb --output /tmp/rui-editor-NEW
python3 fixtures/gtk/incremental_check.py --root fixtures/gtk/editor-golden --report fixtures/gtk/editor-interaction.json
```

RegisteredApplication observes each capture's native size at frame boundaries.
A changed allocation marks the source dirty; unchanged frame ticks produce no
publication. GtkProducerPort.restart_source is a bounded trusted supervisor API,
serialized with source dispatch/capture. It revokes old source authority, retires
identities, switches the native lifecycle fence, and keeps the broker/command ledger
and view scopes. It is not decoded from a peer command. Reacquire explicitly after
the replacement publication. Cancellation before owner execution drops the queued
operation; a completed replacement is never transparently replayed.
