# Native source controls and recovery

The mirror discovers actions, editable text, collection items, CustomCanvas
bindings and source surfaces from the published view. Source resize targets the
focused mapped toplevel; the toolbar sends an explicit authorized ResizeRequest.
Selecting an advertised ScrollTo target enables an absolute source scroll value.
Observer pan, zoom and window size continue to affect only that observer.
The optional `--scale=VALUE` starts a mirror at a finite local scale between
0.125 and 8 (default 1). Requesting control focuses the canvas so subsequent
physical keys reach its input controller.

If rendering or texture allocation exceeds the shared client budget, the mirror
publishes a suspended state and a persistent visible explanation. A previous
frame cannot authorize input after that failure. Reducing the local viewport
can produce a fresh valid frame and clear the rendering notice. Root's five
actual TLS recovery tests include a float-frame allocation that succeeds while
the simultaneous RGBA allocation fails, followed by smaller-viewport recovery;
strict all-target Clippy and formatting also pass.

Canvas drawing requires one advertised compatible pointer device and a current
lease and Presented frame. A stroke retains up to 64 samples, including press and
release, under a reservation acquired before capture. Release submits one ordered
RawInputBatch. Every sample must retain the same source target, routing context
and viewport. Overflow, Escape, changed readiness or invalid geometry cancels the
unsubmitted stroke with visible feedback; no partial batch is sent. The exact
captured sample fingerprint includes order, timestamp, coordinates and button state.

The [native broad report](../fixtures/render/native-broad.json) records a root
review run on an authenticated private Xvfb and private bus. Two real GTK mirrors
match the authoritative source records and full-canvas RGBA after checkbox,
stable-list selection/reorder, scroll, popup/modal/tooltip, canvas and source-size
changes. The controller uses an opaque 32 KiB/s TLS relay; the observer is direct.
The source input trace matches every captured canvas sample. Native overflow and
Escape leave command and source-commit counters unchanged. Both views reacquire
lost resources, and the controller resumes its retained connection with a fresh
actual after-paint callback before regaining control.

This run also exposed and now guards delayed initial publication: a viewport
change cannot render or acknowledge revision zero. A deterministic TLS regression
withholds the first publication, changes the viewport, and verifies that the
first frame uses the latest viewport only after its complete resources arrive.

The retained post-authentication exports contain 806 controller and 601 observer
packets. The independent network auditor checks 54 complete publication prefixes,
all six record kinds, and 150 descriptor-bound vector resource transfers. Exact
summary hashes and counts are replayed in CI. These are explicitly exported
prefixes with reconnect boundaries, not a claim about traffic after export.

Reproduce with the pinned native dependencies and an isolated Xvfb binary:

```sh
cargo build --offline -p rui-client-desktop --features desktop --example corpus_smoke
python3 fixtures/render/native_smoke.py --xvfb /path/to/Xvfb --probe corpus \
  --report /tmp/rui-native-broad.json
```

The GTK producer's guarded wire composition has an
[accepted source trace](gtk-coverage.md#native-wire-composition-producer), and the
[actual desktop wire run](gtk-coverage.md#actual-desktop-wire-composition) measures
native IM callbacks, private preedit, scalar commits and Escape cancellation with
real after-paint readiness. The mirror selects this path only for an advertised
`text.client.v1` view and a lease permitting TextComposition. Semantic ReplaceText
is the fallback when composition is unadvertised; a failed wire stage never uses it.
Commit confirmation requires the matching Applied result, exact source text/caret
and the editing Surface's causal capture token before a fresh native presentation
can authorize the next edit. GTK's optional negotiated clock profile preserves its
observed clear/insert/caret transitions.

Wire editing failures clear local preedit and stop the original grant through a
priority cleanup lane even when the ordinary GUI queue is full. Recovery requires
fresh manual control. Fresh grants retire prior editing generations, and delayed
revocation notices cannot stop an unrelated newer lease. The source fences old
authority immediately and clears transient state on its native thread.
Native session physical input passes the baseline, cleanup and routing/scope
measurements below. This controlled-source run measures real after-paint callbacks
without a physical scanout claim.

The desktop physical-input path recognizes an explicitly granted
`raw.host.v1`/`HOST_PHYSICAL` device in the `linux.evdev` namespace.
`rui-mirror --session` requests the exact whole-session selection; reconnect
preserves that selection. Each actual painted frame retains a charged routing
summary, bounded to 128 surfaces and 64 input rectangles per surface. A key or
pointer event captures its target, focus, geometry and route from that summary.
The worker rejects changed focus, movement, modality and occlusion before
encoding a command; a Partial or unrouteable top surface cannot expose a
clickable surface underneath. Ordinary visual-only updates can advance while
the same route remains valid, using the broker's actual Presented revision.

Key capture preserves physical identity through release and suppresses client
autorepeat. Potential held input remains owned until its matching dispatch
result. A separate grant-scoped stop signal cancels even an already-polled,
backpressured socket write without discarding the exact command queue. Recovery
returns to observation with a persistent request-for-fresh-control notice;
physical safety stops do not automatically reacquire a lease.

A newer actual paint can arrive before the broker acknowledges its Presented
marker. Input can still use the older acknowledged marker only when the complete
routing summaries agree across that marker, the captured paint and current state.
This includes focus clocks, coverage, canvas, every surface's geometry, clips,
transforms, input regions, modality and stacking. The wire retains the actual
acknowledged revision; a changed route requires a fresh presentation.

Root's frozen review passed eight physical preparation tests, five actual TLS
recovery tests, strict all-target Clippy and formatting. The new tests cover
visual-only progression and reject changed routing at either comparison boundary,
wrong connection identity and future paint revisions. The subsequent native
source/two-mirror measurements below establish the declared physical session scope.

The [native session baseline](../fixtures/session-shell/native-session.json)
uses two actual TLS mirrors and their native EIS source on an authenticated
private display. Both actual after-paint record digests and complete viewport
pixels match direct authoritative captures. A physical US Y key produces German
host z with host repeat and exactly one native press/release pair. Pointer and
vector cursor use distinct logical, granted EIS and framebuffer geometry. Native
popup, tooltip, modal, configure and local takeover changes converge; all shared
parent charges return to zero after disposal. Separate
[held-input verification](../fixtures/session-shell/native-held-verification.json)
covers disconnect, expiry, approval revocation and local takeover, including
native releases/STOP and observer convergence. The
[mapping follow-up](../fixtures/session-shell/native-mapping.json) compares a real
remote TLS/EIS click with a direct GDK source click after local pan and fractional
scale. The [wire routing/scope follow-up](../fixtures/session-shell/native-wire.json)
uses nine actual paints from a third receiver to establish valid input before
move, occlusion and modal changes. Each old route and exact retry is rejected
without source effects. Application-only scope closes before projection, and a
fresh manual generation performs one source effect after local takeover. The
third receiver and source owner return their reservations to zero.

```sh
cargo build --offline --locked -p rui-session-shell --features native-mirrors --example native_session_smoke
python3 fixtures/session-shell/native_session.py --xvfb /path/to/Xvfb \
  --target-dir target/debug --report /tmp/rui-native-session.json
```
