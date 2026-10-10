# Isolated session shell profile

`rui-session-shell` owns a built-in vector input laboratory.
Its native-eis feature creates an EIS server and a libei sender joined by an owned private fd.
No native fd, compositor connection, portal token, evdev device, or unrestricted injection service is exposed to a wire peer.
This is an isolated RUI session, not support for the user's desktop or arbitrary applications.

The native gate pins libei/libeis 1.6.0 and xkbcommon 1.13.2 at build time.
The report also hashes the **actual compiled XKB keymap bytes**, including the final NUL supplied to the EIS keymap fd.
This records the installed keyboard-data result rather than inferring it from the library version.
The optional feature requires these system development libraries; default builds exercise topology without loading them.
Native tests are explicit ignored gates, never conditional successful skips.

## Admission and delivery

Only `InjectionScope::IsolatedWholeSession` is supported.
Application-only injection is denied because this backend does not enforce an application sandbox inside the isolated session.
`Backend::submit_raw` admits exact bytes through an injected `SessionBroker` and `CommandLedger`.
An integrating host must use its existing instances for every command family.
`validate_source` plus `dispatch_admitted` is the separate trusted callback boundary for a host that has already admitted through that same ledger; it must not admit again.

The current source-issued route, complete core input preflight, supported native keys/buttons, hit test and explicit granted-region bounds are validated **before native START or any event send**.
The batch revision must equal the decoded observed revision and the broker's actual Presented marker.
Possession of a presentable revision alone is insufficient.
The seat/device names are exactly `rui-isolated-seat` / `rui-private-device`; physical keys use `linux.evdev`.
Button codes 1–8 map respectively to Linux BTN_LEFT, BTN_RIGHT, BTN_MIDDLE, BTN_SIDE, BTN_EXTRA, BTN_FORWARD, BTN_BACK and BTN_TASK.
Other button codes fail closed.
Wheel deltas use libei logical-pixel deltas.

Each protocol event becomes one native hardware frame.
Button press and wheel frames first position the pointer; a release sends only the release and does not move the pointer to an unauthorized cleanup coordinate.
Reliable pointer and touch samples are not coalesced.
A lease has a monotonically increasing native emulation sequence; source route changes while held input exists cause neutralization and a new sequence.
Client timestamps are validated for order, but native frames use the native monotonic clock, not client timestamps.

A bounded FIFO stores trusted command sequence, event index, native sequence, lease generation, source route and exact expected event fields.
Receiving EIS frames must match this FIFO, including mapped native touch identities.
Before source mutation the receiver checks current lease, route, focus, modality and fresh-point hit test again.
Captured button motion and matching touch motion may continue outside their surface, within the whole-session grant.
Fresh presses, touches and wheel events must hit the actual topmost input-enabled surface.

The command's stable result is **Dispatched**.
Actual native receipt and source mutation are observed separately through a bounded metadata trace; they do not retroactively turn injection into Applied evidence.
Exact ledger retries never send again.
Source text is not written to the trace.

## Source topology and geometry

The source supports up to 16 windows and 4096 retired record IDs.
Toplevel, popup, tooltip and shell roles are represented by actual Surface, Scene, Semantic, Binding and Session records.
Each window has a stable six-record incarnation; reopening allocates fresh IDs.
Closing a parent closes descendants.
Opening does not steal focus; the trusted source explicitly focuses permitted windows.
The highest mapped modal blocks input to all windows except itself and its descendants.
Closing it restores the permitted parent focus.
Tooltips have no input region and cannot receive focus.
Unmapping a parent also unmaps its descendants and clears focus within that subtree.

Surface placement is `T(desktop_rect.x,y) * transform`; parent links add no coordinate offset.
Content bounds are the local clip.
Desktop rectangle sizes are metadata, not implicit rescaling.
Source-authorized configuration changes move, content size, transform and stacking atomically after core validation, and invalidate routing tokens.
Input hit testing uses inverse transforms and the same `(stacking_order,id)` order as reference surface composition.

Logical canvas, granted EIS region, physical scale and framebuffer dimensions are declared independently.
Only the first two define input mapping.
The native probe deliberately uses logical 200×120, grant (40,80,600,240), scale 2 and framebuffer 1000×700.
Out-of-grant input is denied before libei can silently drop it.
Cursor position comes from received native motion and projects through the registered `cursor.vector.v1` renderer profile; a network producer publishing visible cursors must negotiate that profile.

The laboratory appends native XKB key output to a bounded **public** semantic value and changes a vector activity indicator for native events.
It does not claim text glyph rendering, host IME composition, arbitrary toolkit behavior or application business logic.
Surface capture tokens remain stable when their payload is unchanged, while cursor-only updates change Session.
Actual source records are replay-tested through the core reducer across cursor/activity updates.
Direct source frames have independent asserted interior pixel colors, transforms, clips, stacking and hit-test geometry.

## Keyboard ownership and cleanup

The host uses actual native XKB with fixed `evdev`/`pc105` rules and explicit `us` or `de` layout.
The probe declares a US client layout assumption and shows physical evdev code 21 produces German `z` from the host keymap.
Client-generated repeat presses fail core transition validation, and a corresponding client text commit is not accepted by this physical-key backend.
Repeat is source-owned: 400 ms delay, 40 ms interval, at most 32 repeats per pump, with current lease/focus/route and actual Presented/resource-readiness checks.
Losing readiness neutralizes the native device, clears held/repeat state after matched release frames and STOP, and requires a fresh keydown after readiness returns.
It does not consume unrelated common cleanup or revoke another family's context.
Source repeat time is the host-supplied monotonic time.
It is unrelated to the client's event timestamp or keyboard layout.

`local_input` is the shell's trusted local handler: it revokes the common lease first, cancels source repeats and queued effects, sends real key/button releases and touch cancels, matches every receiving native cleanup frame, then requires STOP_EMULATING and neutral source/XKB state.
Idle local input needs no native STOP.
Route changes while held input exists take the same neutralization path.
Confirmed ordinary releases remove matching held state from the broker.

The backend **never consumes common InputCleanup records**.
The integrating host must fan them out to every family, including composition owners, and acknowledge only after all required cleanup completes.
A new grant stays blocked until that central acknowledgement.
A cleanup failure keeps that ownership pending.
Ambiguous START/send and any native/source pump failure poison fresh dispatch.
Successful verified cleanup can recover known held state; untracked native failure requires disposal.
Already executed source effects are not rolled back.

## Bounds and reproducible gate

Normal pending work is at most 512 frames, with at most 128 additional cleanup frames.
There are at most two native events per frame and 256 retained traces.
A pump handles at most 2048 native events.
Source public values are at most 4096 UTF-8 bytes/window.
Setup and cleanup each have a two-second deadline.
Shared memory reservations precede allocation: 2 MiB for trusted native opaque state, 4 MiB for bounded live topology, 1 MiB for backend queues/state, and 2 MiB staging for source projection.
Escaped source projections retain their own actual conservative charge.
Native allocator RSS is not instrumented by this ledger; the opaque native allowance is explicit, not an allocator-level RSS proof.

```
cargo test -p rui-session-shell --offline --locked
cargo test -p rui-session-shell --features native-eis --test native --offline --locked -- --ignored
cargo run -p rui-session-shell --features native-eis --example native_probe --offline --locked -- /tmp/session-shell.pam
```

The fd-only native probe renders the actual source records into a local PAM and prints a hash-only JSON report.
It uses trusted admission readiness fixtures; it does not claim actual desktop mirror Presented, a network round trip, or physical scanout.
Its local takeover enters the real shell handler and verifies native neutralization.
It predates the separate actual GDK source-window gate below.
The retained report is `fixtures/session-shell/native-source.json`.

## Native owner and local source window

The optional owner actor constructs the native contexts, authoritative topology, one SessionBroker and one CommandLedger on its named owning thread.
Callers get an opaque connection and a Send port, never a native context.
Normal requests have a 32-entry bounded queue and one-entry charged reply channels.
Captures are borrowed direct-source projections; checkpoints encode those same records.
Retained checkpoint bodies are prepared before changing the broker view.
Escaped projection and immutable publication handles retain their global charges after owner shutdown, including cloned publication handles.

Each connection capability binds the owner identity and a monotonically issued connection serial as well as protocol client/view/epoch.
Every queued operation rechecks the current serial on its owning thread.
Same-view reconnect cannot reuse an old capability, and late old disconnects cannot overwrite or disconnect a newer incarnation.
Cross-owner capabilities fail even when trusted fixtures explicitly use identical protocol IDs.
A standalone owner mints fresh OS-random session and epoch IDs; fixed IDs are available only through the explicit trusted fixture/deployment constructor.
A trusted deployment must issue a new epoch when replacing broker or ledger state.
Dropped authentication replies disconnect their exact registration, including a queued reply never consumed by its caller.
Connection storage reserves before broker negotiation.
Negotiation returns the exact retained checkpoint keeper atomically for a zero-delta resume, even if current source records have since changed.
Resume clears presentation/control; the integrating mirror must provide new actual presentation evidence.

Local input is a separate trusted capability that cannot be selected by a wire message.
Its 64-entry fixed-event FIFO records a checked outstanding obligation before invalidation and enqueue.
Priority generation checks cancel already-sent native work at receipt.
Raw and acquire requests also require the owner to have acknowledged the exact generation after lifecycle handling, verified cleanup and local FIFO processing.
A new-generation request cannot overtake an unprocessed local event.
A failed enqueue still invalidates remote authority; earlier accepted local events finish before an explicit poisoned status denies fresh dispatch.
Capture/status remain usable for reporting the safe or failed source.

Native cleanup is incremental: each owner iteration receives bounded native events without sleeping on a native wait.
The owner continues handling priority local/disconnect/quit notices while actual releases, cancels and STOP arrive.
Eight fixed disconnect slots are exact client/view bound; observer disconnect does not revoke the controller.
Quit is terminal and cannot be blocked by normal queue pressure.
Startup failures join and dispose the native owner.
The actor currently grants only RawInputBatch, so its central cleanup coordinator can acknowledge after this backend is neutral; an unexpected composition cleanup is fail-closed.
A multi-family integration must replace that narrow fanout with all participating owners.

The actor reserves an 8 MiB allowance for bounded broker metadata, a 24 MiB ledger allowance, and a 1 MiB request/priority allowance before creating maps or queues.
The metadata allowance covers at most eight client slots, their bounded policy and negotiation strings, 1024 control request identities and 128 held inputs; it excludes publication bodies and view allocations.
Both native-only and transport builds use `SessionBroker::with_memory_budget`: each decoded checkpoint, retained publication, history/outbound entry, attachment and escaped socket reservation owns a charge against its view and the same aggregate parent.
Those charges outlive owner shutdown until the last allocation handle drops.
The owner retains its metadata/ledger allowance until covered maps are destroyed.
Native-only views have a 16 MiB local ceiling.
Transport-native views have a 32 MiB ceiling so a normal bounded GTK viewport can retain its linear float frame while constructing a charged RGBA texture and socket writer buffers.
All charges still belong to the same aggregate parent budget; increasing the per-view ceiling does not allocate an independent pool.
Both profiles retain 1 MiB publication/history ceilings, 64 history entries and 512 KiB outbound queues.
The ledger has at most 4096 identities, 8 MiB retained exact command bodies and 512 retained outcomes.
Native/backend/ topology charges remain those above.
This is a conservative explicit allocation profile, not an allocator-level RSS proof.

`source-window` adds a private GTK frontend.
It currently requires explicit authenticated isolated X11 with the verified hardware-keycode = evdev + 8 mapping; other GDK backends are unsupported.
Actual GDK key/motion/click callbacks enter the local priority capability before source work.
Physical keys are interpreted by the source's native XKB state, with no client text commit or IME substitution.
The vector canvas scales uniformly with letterboxing; inverse pointer mapping rejects letterbox pixels, then the independent granted EIS region maps source logical coordinates.
Native platform cursor painting is hidden over this canvas; its visible cursor is the explicit vector Session cursor.
GTK diagnostics/chrome outside the canvas are local and not projected.

Canvas focus loss, unmap, close and frontend disposal send a separate priority local-device cancellation.
It cannot be blocked by a full FIFO, clears native XKB/local held state after remote cleanup, and invalidates old queued local events by their local-device generation.
Teardown removes controllers, timer, after-paint handler, texture and window content.
The shared opaque native allowance remains attached to native object clones until their final drop.

The frontend renders the actor's actual borrowed source state.
Its linear frame is transferred over a white linear matte into budget-owned RGBA bytes retained by GLib.Bytes/MemoryTexture.
It reserves an explicit 4 MiB GTK/GDK allowance; native allocator/GPU RSS is not instrumented.
Local allocation/queue/capture failures keep visible feedback across later frames.
Diagnostic observation is default-off and stores one fixed-size metadata value with hashes/revision only.
The observed stamp is the one submitted by Widget.snapshot and subsequently seen at GTK after-paint.
This proves a source-window presentation opportunity, not physical scanout or a remote RUI PresentedAck.

```
cargo test -p rui-session-shell --features source-window --offline --locked
cargo test -p rui-session-shell --features source-window --offline --locked -- --ignored
cargo build -p rui-session-shell --features source-window --example source_window_probe --offline --locked
python3 fixtures/session-shell/native_window.py --xvfb /path/to/Xvfb --report /tmp/source-window.json
```

The private runner owns authenticated Xvfb and its DBus session.
The probe starts with an actually received remote Shift held, then delivers real private-display XTest events through the same GDK callbacks used by the source frontend.
A US client physical y key becomes German z, after the remote held modifier has been released.
It checks actual local pointer mapping, native held state/STOP cleanup, direct-source interior pixels, and exact source/pixel hashes for the revision observed at after-paint.
Four source surfaces compose into a 200×120 frame.
It then holds an actual native key, moves native focus to another private window, and proves local held state is cleared before the otherwise missing native release.
The source stays unchanged for 500 ms before that test key is released.
The probe also retains window/canvas clones across frontend disposal and checks that controller/content teardown occurs and the opaque native allowance releases only after those clones are dropped.
The retained hash-only report is `fixtures/session-shell/native-window.json`.
Trusted test acknowledgements admit the initial raw command; this probe does not claim a remote mirror, network transport or actual physical keyboard device.

## Optional authenticated producer port

`transport` adds `actor::transport::NativeProducerPort`, an implementation of the common compound `rui_transport::producer::ProducerPort`.
It queues requests on the existing native owner; that owner retains the same topology, broker, ledger and native contexts.
Negotiation and exact resume keeper construction are one operation.
Scope/connection incarnation and immutable pairing approval are rechecked when work executes.
Time is sampled on the owner after queue waiting.
There are at most four connected protocol clients.
Only the explicit `selection="session", mirror_canvas_policy="host-canvas"` export is supported; fixture and application-only selections are denied.
Raw-capable views advertise the exact seat/device and `raw.host.v1`/`HOST_PHYSICAL` metadata.
Other command families receive stable unsupported rejection through the common ledger.

Publication clocks belong to each view.
Multiple native changes before a pump become one legal replacement increment per changed record; unchanged final fields do not invent mutations.
Focus reference changes advance the view focus clock exactly once.
Private native focus/route clocks and capture tokens remain authoritative for dispatch, including focus ABA.
`projected_capture` returns direct normalized source records at the last published view revision under retained allocation charges; it does not reconstruct the source from wire bytes.
Graph count/depth, publication bytes and decoded memory must fit the negotiated receiver ceilings before checkpoint installation or delta publication.
The immutable checkpoint body is prepared before installation, so allocation pressure cannot leave a new broker checkpoint paired with an old source capture.

Asynchronous calls use the fixed normal FIFO and bounded single-response channels; saturation returns an explicit limit error.
No task is spawned per request.
Disconnect, local takeover and shutdown remain independent priority obligations continuously serviced by the native owner.
A lifecycle flush waits for queued reply ownership to settle, priority generation acknowledgement and verified native cleanup; a saturated normal queue cannot erase cleanup.
Cancelled authentication replies disconnect the exact issued registration.
Actual remote Presented acknowledgements enter the same broker and remain required for grants.

```
cargo test -p rui-session-shell --features transport --offline --locked
cargo test -p rui-session-shell --features transport --offline --locked -- --ignored
cargo clippy -p rui-session-shell --features source-window,transport --all-targets --offline --locked -- -D warnings
```

The port unit/native tests use explicit trusted receipt fixtures.
The optional `native-mirrors` example now passes actual TLS/two-mirror presentation and native physical callbacks against the same source owner, with exact records/pixels, host layout/repeat, unequal coordinate mapping, topology/cursor and local takeover.
See the [native baseline](../fixtures/session-shell/native-session.json), [held-input cleanup matrix](#actual-held-input-cleanup-matrix), and [stale-route/scope measurements](#actual-stale-route-and-scope-rejection) for the accepted declared backend.
Host IME and remote source resize are unsupported by this `raw.host.v1` port and are not advertised.
Controlled-host resize and broader M2 controls are assessed separately from this isolated native backend.

The isolated vector source applies one shared button-press operation from native EIS input and local GDK input.
Ordinary releases and authority cleanup release frames neutralize held input without activating the application again.
Root's 43 actual native-feature tests, strict all-target Clippy and formatting pass, including equal local/native press effects, a real cleanup release and explicit old-generation Protobuf rejection with no new native frame.
The subsequent [mapping report](../fixtures/session-shell/native-mapping.json) measures a real remote TLS/EIS click against a direct GDK source click after local pan and fractional scale; the two normal mirrors match their own source records and pixels.

Trusted owner diagnostics retain only the latest completed cleanup reason, lease generation and completion time, plus the current lease generation/deadline and native work generation.
These fixed-size observations contain no lease capability or text and allow probes to distinguish expiry from disconnect.


## Actual held-input cleanup matrix

Root independently rebuilt the frozen native session probe and passed four separate private-display runs, retained in `fixtures/session-shell/native-held-*.json`.
The source first receives one real remote key press and one button press.
An opaque controller-only TLS relay then cuts the socket or stops renewal; the other variants revoke local pairing approval or deliver an actual source-window GDK click.
Authoritative diagnostics require the original lease generation and respectively Disconnect, Expired, AuthorizationChanged or LocalActivity.
Expiry occurs no earlier than the recorded deadline while the original connection and approval remain live; local takeover also advances native work generation.

Every run observes both native cleanup release frames, zero held state and a Ready owner after the receiving EIS STOP.
Old physical releases have no new native effect.
The healthy observer's complete records and actual after-paint pixels equal its same-owner source projection.
Non-revoked clients recover only after a fresh manual grant and a new physical press/release; no held state is restored.
All frontend/server/owner charges return to zero after disposal.

The harness waits for the broker's actual Presented marker before its one-shot Request Control click, rather than treating a local after-paint callback as a network acknowledgment.
That trusted read requires a current connected, opened, approved view and cannot create a marker.
Private bus configuration has no service activation directories.
Owned process groups always terminate, driver panic requests GTK shutdown, and child logs retain at most 64 KiB.
The source text preview wraps long runs and ellipsizes after three lines without truncating the authoritative text model.
Root's 43 native tests, panic-shutdown regression and strict native all-target Clippy pass alongside the four actual runs.

```sh
cargo build --offline --locked -p rui-session-shell --features native-mirrors --example native_session_smoke
python3 fixtures/session-shell/native_session.py --xvfb /tmp/rui-xvfb/usr/bin/Xvfb --matrix disconnect --report /tmp/rui-held-disconnect.json
```

Repeat with `expiry`, `revoke` and `local-takeover`, each with a fresh report path.
This declared backend remains an isolated exported session, not arbitrary personal desktop input or physical scanout evidence.


### Actual stale-route and scope rejection

`fixtures/session-shell/native-wire.json` records root's independent frozen run.
Two normal mirrors stay live while a third authenticated receiver renders its own validated source projection.
Its GTK snapshot and actual frame-clock callback provide nine distinct Presented witnesses; it borrows no other view's marker.
Each move, occlusion and modal case first proves a valid native click with exactly one source effect.
The probe then captures immutable R1 bytes, mutates the source route, and sends those bytes through TLS.
The adapter maps its routing rejection to `Rejected InvalidArguments`; exact retries agree and both native frame counters and authoritative text/activity remain unchanged.

An authenticated application-only Open closes before any projection or resource delivery.
A separate connection explicitly opens the supported whole session.
Actual local GDK activity then revokes the original generation: its old wire command is `Rejected StaleControl` without effects.
A separate manual fresh grant dispatches once, with two EIS press/release frames and one source effect; exact retry adds neither.
Both normal mirrors match their own source records and pixels.
The third receiver/GTK budget and the parent owner return to zero.

```sh
python3 fixtures/session-shell/native_session.py --xvfb /path/to/Xvfb \
  --routing wire --report /tmp/rui-native-wire.json
```

These results complete the declared isolated-backend M4 integration gates.
They do not extend support to an arbitrary desktop or unexported application.
Final accepted-code reruns retain the [native baseline](../fixtures/session-shell/native-session-final.json) and [held local-takeover cleanup](../fixtures/session-shell/native-held-local-takeover-final.json) separately from these earlier reviewed reports.
