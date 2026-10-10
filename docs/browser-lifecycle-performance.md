# Browser lifecycle measurements

The [three retained runs](../fixtures/browser/lifecycle/summary.json) exercise the
installed private 0.1.1 tarball outside the checkout against the owned GTK corpus,
production HTTPS and a certificate-verifying WSS proxy. Each raw report records
hardware, compiler, native/browser versions, source/binary hashes, a 1200×1200
viewport at scale 1 and loopback conditions. The environment is i7-1280P,
Chromium 151.0.7922.173, Rust 1.98.1, GTK 4.22.4 and Cairo 1.18.4.

Build the release demo and WASM first, then run without concurrent builds/tests:

```sh
RUI_WASM_CHROMIUM=/path/to/chromium python3 ci/measure-lifecycle.py \
  --xvfb /path/to/Xvfb --output /tmp/rui-lifecycle-NEW
```

The fixture installs, typechecks and builds its isolated consumer before starting
the host or measured intervals. Ordinary pinned GTK CI runs one correctness check
with `RUI_LIFECYCLE=1` and generous deadlines; the controlled performance workflow
retains three runs. Timings finish on the public visible-frame event, after verified
resources and zero pending commands. GC/state comparisons occur outside those
intervals. Cold initialization includes WASM fetch/compilation and connection but
excludes HTML/JS download. Subsequent actor creation uses initialized WASM.

| Observation | Three-run range, milliseconds |
| --- | ---: |
| Initial cold connection, one sample/run | 291.9–301.7 |
| Disconnect/reconnect p95, three samples/run | 252.4–257.7 |
| Resource-cache loss to presentation p95, three samples/run | 232.6–235.1 |
| Held command-result recovery, one sample/run | 232.5–240.0 |
| Repeated actor connection p95, 25 samples/run | 258.4–262.2 |

GTK reconnect requires a fresh-scope snapshot; it does not establish retained GTK
delta resume. Only the authenticated producer-busy response retries for at most two
seconds; its 50 ms retry waits remain inside the timing interval and reports count
them. The same actor retains exact pending command bytes across reconnect. The
test holds one Applied result while accepting its authoritative publication, then
reconnects and queries that outcome without issuing a fresh edit or regaining
control. The command ledger is empty at measured completion.

Each run disposes/recreates 25 actors. Rust and Canvas ownership return to zero on
every disposal. Post-GC backing storage remains 9,386,289 bytes; each sample has one
document, 22 DOM nodes and 18 event listeners. JS heap rises only 19,384–38,764 bytes
between cycles 5 and 25. The stabilization check allows at most 8 MiB JS/backing
storage drift and 64 additional nodes, with unchanged listener count. These are
explicit observational checks, separate from negotiated Rust/resource/queue limits.
Rust high-water includes conservative transient ownership, not browser allocator
capacity; the renderer reports its own independent allocation budget.

A real headed-tab visibility transition releases control, refills verified
resources while hidden, suppresses presentation for the hidden interval, and
presents after return without reacquiring authority. Reports retain those checks,
wire-byte counts, bounded stage samples and peak ownership. They contain no
application text, credentials or protocol payloads.

Sparse lifecycle samples do not establish the 200-action warm targets, general
60 Hz performance, physical scanout, physical mobile support or two-machine
tailnet acceptance. The [warm gates](performance-gates.md) keep their original
thresholds and measured limitations.
