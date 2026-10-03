# Native source controls and recovery

The mirror discovers actions, editable text, collection items, CustomCanvas
bindings and source surfaces from the published view. Source resize targets the
focused mapped toplevel; the toolbar sends an explicit authorized ResizeRequest.
Selecting an advertised ScrollTo target enables an absolute source scroll value.
Observer pan, zoom and window size continue to affect only that observer.

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

Native GTK producer wire composition and native session physical input remain
separate integration gates. This controlled-source run measures real after-paint
callbacks; it does not establish physical scanout timing.
