# Isolated session shell native foundation

`rui-session-shell` owns a built-in vector input laboratory. Its native-eis
feature creates an EIS server and a libei sender joined by an owned private fd.
No native fd, compositor connection, portal token, evdev device, or unrestricted
injection service is exposed to a wire peer. This is an isolated RUI session,
not support for the user's desktop or arbitrary applications.

The native gate pins libei/libeis 1.6.0 and xkbcommon 1.13.2 at build time. The
report also hashes the **actual compiled XKB keymap bytes**, including the final
NUL supplied to the EIS keymap fd. This records the installed keyboard-data
result rather than inferring it from the library version. The optional feature
requires these system development libraries; default builds exercise topology
without loading them. Native tests are explicit ignored gates, never conditional
successful skips.

## Admission and delivery

Only `InjectionScope::IsolatedWholeSession` is supported. Application-only
injection is denied because this backend does not enforce an application sandbox
inside the isolated session. `Backend::submit_raw` admits exact bytes through an
injected `SessionBroker` and `CommandLedger`. An integrating host must use its
existing instances for every command family. `validate_source` plus
`dispatch_admitted` is the separate trusted callback boundary for a host that
has already admitted through that same ledger; it must not admit again.

The current source-issued route, complete core input preflight, supported native
keys/buttons, hit test and explicit granted-region bounds are validated **before
native START or any event send**. The batch revision must equal the decoded
observed revision and the broker's actual Presented marker. Possession of a
presentable revision alone is insufficient. The seat/device names are exactly
`rui-isolated-seat` / `rui-private-device`; physical keys use `linux.evdev`.
Button codes 1–8 map respectively to Linux BTN_LEFT, BTN_RIGHT, BTN_MIDDLE,
BTN_SIDE, BTN_EXTRA, BTN_FORWARD, BTN_BACK and BTN_TASK. Other button codes fail
closed. Wheel deltas use libei logical-pixel deltas.

Each protocol event becomes one native hardware frame. Button press and wheel
frames first position the pointer; a release sends only the release and does
not move the pointer to an unauthorized cleanup coordinate. Reliable pointer
and touch samples are not coalesced. A lease has a monotonically increasing
native emulation sequence; source route changes while held input exists cause
neutralization and a new sequence. Client timestamps are validated for order,
but native frames use the native monotonic clock, not client timestamps.

A bounded FIFO stores trusted command sequence, event index, native sequence,
lease generation, source route and exact expected event fields. Receiving EIS
frames must match this FIFO, including mapped native touch identities. Before
source mutation the receiver checks current lease, route, focus, modality and
fresh-point hit test again. Captured button motion and matching touch motion may
continue outside their surface, within the whole-session grant. Fresh presses,
touches and wheel events must hit the actual topmost input-enabled surface.

The command's stable result is **Dispatched**. Actual native receipt and source
mutation are observed separately through a bounded metadata trace; they do not
retroactively turn injection into Applied evidence. Exact ledger retries never
send again. Source text is not written to the trace.

## Source topology and geometry

The source supports up to 16 windows and 4096 retired record IDs. Toplevel,
popup, tooltip and shell roles are represented by actual Surface, Scene,
Semantic, Binding and Session records. Each window has a stable six-record
incarnation; reopening allocates fresh IDs. Closing a parent closes descendants.
Opening does not steal focus; the trusted source explicitly focuses permitted
windows. The highest mapped modal blocks input to all windows except itself
and its descendants. Closing it restores the permitted parent focus. Tooltips
have no input region and cannot receive focus. Unmapping a parent also unmaps
its descendants and clears focus within that subtree.

Surface placement is `T(desktop_rect.x,y) * transform`; parent links add no
coordinate offset. Content bounds are the local clip. Desktop rectangle sizes
are metadata, not implicit rescaling. Source-authorized configuration changes
move, content size, transform and stacking atomically after core validation,
and invalidate routing tokens. Input hit testing uses inverse transforms and
the same `(stacking_order,id)` order as reference surface composition.

Logical canvas, granted EIS region, physical scale and framebuffer dimensions
are declared independently. Only the first two define input mapping. The native
probe deliberately uses logical 200×120, grant (40,80,600,240), scale 2 and
framebuffer 1000×700. Out-of-grant input is denied before libei can silently drop
it. Cursor position comes from received native motion and projects through the
registered `cursor.vector.v1` renderer profile; a network producer publishing
visible cursors must negotiate that profile.

The laboratory appends native XKB key output to a bounded **public** semantic
value and changes a vector activity indicator for native events. It does not
claim text glyph rendering, host IME composition, arbitrary toolkit behavior or
application business logic. Surface capture tokens remain stable when their
payload is unchanged, while cursor-only updates change Session. Actual source
records are replay-tested through the core reducer across cursor/activity
updates. Direct source frames have independent asserted interior pixel colors,
transforms, clips, stacking and hit-test geometry.

## Keyboard ownership and cleanup

The host uses actual native XKB with fixed `evdev`/`pc105` rules and explicit
`us` or `de` layout. The probe declares a US client layout assumption and shows
physical evdev code 21 produces German `z` from the host keymap. Client-generated
repeat presses fail core transition validation, and a corresponding client text
commit is not accepted by this physical-key backend. Repeat is source-owned:
400 ms delay, 40 ms interval, at most 32 repeats per pump, with current
lease/focus/route and actual Presented/resource-readiness checks. Losing
readiness neutralizes the native device, clears held/repeat state after matched
release frames and STOP, and requires a fresh keydown after readiness returns.
It does not consume unrelated common cleanup or revoke another family's context.
Source repeat time is the host-supplied monotonic time.
It is unrelated to the client's event timestamp or keyboard layout.

`local_input` is the shell's trusted local handler: it revokes the common lease
first, cancels source repeats and queued effects, sends real key/button releases
and touch cancels, matches every receiving native cleanup frame, then requires
STOP_EMULATING and neutral source/XKB state. Idle local input needs no native
STOP. Route changes while held input exists take the same neutralization path.
Confirmed ordinary releases remove matching held state from the broker.

The backend **never consumes common InputCleanup records**. The integrating host
must fan them out to every family, including composition owners, and acknowledge
only after all required cleanup completes. A new grant stays blocked until that
central acknowledgement. A cleanup failure keeps that ownership pending.
Ambiguous START/send and any native/source pump failure poison fresh dispatch.
Successful verified cleanup can recover known held state; untracked native
failure requires disposal. Already executed source effects are not rolled back.

## Bounds and reproducible gate

Normal pending work is at most 512 frames, with at most 128 additional cleanup
frames. There are at most two native events per frame and 256 retained traces.
A pump handles at most 2048 native events. Source public values are at most 4096
UTF-8 bytes/window. Setup and cleanup each have a two-second deadline. Shared
memory reservations precede allocation: 2 MiB for trusted native opaque state,
4 MiB for bounded live topology, 1 MiB for backend queues/state, and 2 MiB staging
for source projection. Escaped source projections retain their own actual
conservative charge. Native allocator RSS is not instrumented by this ledger;
the opaque native allowance is explicit, not an allocator-level RSS proof.

```
cargo test -p rui-session-shell --offline --locked
cargo test -p rui-session-shell --features native-eis --test native --offline --locked -- --ignored
cargo run -p rui-session-shell --features native-eis --example native_probe --offline --locked -- /tmp/session-shell.pam
```

The native probe renders the actual source records into a local PAM and prints
a hash-only JSON report. It uses trusted admission readiness fixtures; it does
not claim actual desktop mirror Presented, a network round trip, or physical
scanout. Its local takeover enters the real shell handler and verifies native
neutralization, but native local-device capture remains a later integration gate.
The retained report is `fixtures/session-shell/native-source.json`.

This slice is a source/native foundation. M4 still requires the shared transport
host raw-family hook, actual mirror presentation, owned native local-input
capture, and host IME/resize integration. Full M2 control/transient/recovery gates
remain separate from this probe.
