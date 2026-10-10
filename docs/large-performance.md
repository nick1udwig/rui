# Frozen large-workload and recovery measurements

The [repeated reports](../fixtures/performance/large-p1/summary.json) retain the
original release CPU/TLS fixture: 10,000 meaningful records, 2,500 visible native
glyph cells, 1000×600 canvas at scale 1, two healthy clients and one reader stopped
after its initial checkpoint. Each one-record/twenty-record phase has ten warmups
and 100 measured updates. The original host/receiver 128 MiB, renderer 256 MiB and
outbound queue 65,536-byte ceilings remain unchanged.

Build before measuring, then keep other builds/tests stopped:

```sh
cargo build --release -p rui-transport --example measure_integrated --locked
python3 ci/measure-large.py --output /tmp/rui-large-NEW
```

The runner requires 3–10 runs, verifies fixed workload/sample populations and
ownership/resource/idle/disposal assertions, and gives each process a 1200-second
deadline. Raw reports contain each measured sample, per-client distributions,
bounded native stage timings and whole-process peak RSS. The summary records
compiler, kernel, GTK/Cairo/Pango versions and binary/source/font-provider hashes.
This is a named local development-machine observation, not evidence that the
remote dedicated CI runner has been provisioned.

All three runs pass on the i7-1280P, Rust 1.98.1, Linux 7.1.9/glibc 2.44,
GTK 4.22.4, Cairo 1.18.4 and Pango 1.58.2. Each run takes 321.5–323.8 seconds;
the 16,384-entry timing ring drops no samples. The table gives the range of each
run's p95 in milliseconds; raw reports retain p50/p95/p99 and individual samples.

| Measured p95 interval | One record | Twenty records |
| --- | ---: | ---: |
| Publication across connected views | 17.59–18.44 | 11.84–12.66 |
| Source change through both Applied/ACK flushes | 33.61–34.26 | 28.27–29.50 |
| First client CPU render | 711.57–715.06 | 712.46–724.06 |
| Second client CPU render | 713.24–714.27 | 710.22–721.97 |
| Source change through first CPU frame | 741.56–748.27 | 739.75–747.82 |
| Source change through second CPU frame | 1450.04–1474.55 | 1471.57–1473.02 |
| Independent transaction application | 7.99–8.08 | 8.09–8.34 |

Rendering dominates; these observations do not establish a speedup against the
historical single run that included concurrent build/test load. The snapshot is
992,674 bytes, one-record bodies remain 237 bytes, and twenty-record bodies remain
4,323–4,326 bytes. Warm resource-body retransmission and idle publication bytes
are zero in every run. One warm view's total recorded traffic is 1,587,035 bytes.

| Ownership high-water | Observed bytes | Original ceiling bytes |
| --- | ---: | ---: |
| Host warm view | 119,570,962 | 134,217,728 |
| Host including recovery/source comparison | 120,563,894 | 134,217,728 |
| Receiver | 62,346,148 | 134,217,728 |
| Renderer | 268,124,192 | 268,435,456 |
| Sampled outbound queue | 63,978 | 65,536 |

All stopped readers close as SlowConsumer. Replica/renderer disposal returns
tracked ownership to zero and shutdown removes all diagnostic buffers.

Publication time includes projection/encoding across currently connected views.
Per-client Applied ends after its local ACK flush. First-client CPU rendering waits
for both Applied events; second-client rendering is serialized after the first.
The source-change-to-CPU-frame intervals retain that order explicitly, so the second
includes waiting for the first render. Individual render intervals exclude peer
rendering and pixel comparison. The historical two-render aggregate still includes
pixel comparison and remains available for comparison. Independent transaction
application/record checks and final direct-source comparison remain enabled.

Every warm update checks full record/pixel equivalence and resource reuse. The
stalled observer closes independently at its bounded limit, so the phases have
different fanout and do not isolate the cost of changing more records. A final
250 ms idle interval must produce no publication churn. Replica/render ownership
and connection-owned diagnostic buffers release on disposal/shutdown.

Recovery follows the frozen warm measurements and their resource/idle checks:

| Scenario | Checked boundary |
| --- | --- |
| Cold connection, all three clients | TLS/authentication/open/resource verification through Applied; scoped plaintext bytes and receiver high-water |
| Retained zero-delta resume | Same view and records/cache, no resource body, no Presented, empty exact-command ledger |
| Retained resume after cache loss | Same view and records, verified resource refill, no presentation or authority restoration |
| Forced fresh scope | Narrower 49,152-byte envelope forces a fresh snapshot/context without raising limits; exact records match its own authorized source projection |

Recovery rendering compares exact pixels with the healthy peer outside the reported
CPU-ready interval. Fresh scopes restart public clocks; they cannot be compared to
the old view by assuming identical versions. Each recovery reports Applied and CPU
frame times, bytes, resource bodies, pending commands and separate ownership peaks.
The native fixture is read-only; the [browser lifecycle test](browser-lifecycle-performance.md)
separately measures a held command-result recovery.

| Recovery through Applied, three-run observations | Milliseconds | Scoped bytes | Resource bodies |
| --- | ---: | ---: | ---: |
| Retained zero-delta resume | 6.84–8.91 | 441 | 0 |
| Retained resume after cache loss | 13.19–14.95 | 2,012 | 1 |
| Forced fresh scope | 149.44–151.83 | 999,829 | 1 |

These are three observations per recovery scenario, not the 200-action warm
latency population. Recovery CPU renders take approximately 687–699 ms and retain
the same renderer ceiling. Full-state and pixel checks pass after every recovery.

Wire counts cover per-view plaintext envelopes through a post-ACK ping barrier,
including the barrier and replies. They exclude authentication negotiation, TLS and
socket overhead. Diagnostic buffers belong to a connection and are removed at
disconnect, including when resume keeps the view ID; recovery counts the new
connection directly. `measure_integrated --recovery-only` is a short correctness
check that omits warm phases. The repeated measurement runner explicitly rejects
that output as frozen-workload evidence.

CPU frames are linear renderer output, not RGBA8/native submission, scanout or an
actual native Presented event. These reports never replace a presentation
opportunity with Applied. The large 60 Hz goal remains explicit; the original
[small-demo gates](performance-gates.md) retain their separate procedure/targets.
