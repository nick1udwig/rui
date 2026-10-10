# Controlled performance gates

P5 freezes the small-demo procedure in [performance-gate.py](../ci/performance-gate.py).
It requires three release runs, exactly 200 actions per origin after five warmups,
the correctness companion, complete bounded timing rings, Chromium 151.0.7922.173,
GTK 4.22.4/Cairo 1.18.4, Rust 1.98.1, the i7-1280P and the recorded
1200×1200 viewport at scale 0.75. TLS uses the certificate-verifying loopback proxy.
Build all artifacts first and run without competing builds or tests.

The original goals remain p95 ≤100 ms and p99 ≤150 ms per origin. Draw p95
must be ≤16.67 ms: sum preparation, composition, conversion and submission
within each frame, then rank those sums. Never add stage percentiles or replace
the following presentation opportunity with Applied. Action samples include
the harness's 50 ms polling and native event injection. The last 400 complete
frames are the measured actions, following correctness and warmups.

```sh
python3 ci/performance-gate.py fixtures/performance/browser-p4-after
```

The [retained evaluation](../fixtures/performance/browser-p4-after/gates.json)
passes the draw gate in all runs (p95 11.5–12.4 ms) and fails both action gates.
Source callback-to-capture p95 is about 71 ms in the first run, including coherent
capture p95 of 59 ms; the harness adds polling/injection costs. This identifies
capture and the measurement overhead as remaining work, rather than changing
the targets to make the report pass. The milestone's measured scope is the
small constrained GTK demo. The [expanded browser lifecycle](browser-lifecycle-performance.md)
and [three frozen large/recovery runs](large-performance.md) now retain separate
P1 evidence. Large-native CPU render p95 remains 710–724 ms, so its 60 Hz goal
is outstanding. All three large runs preserve zero warm resource bodies, small
updates, original ceilings and bounded slow-client isolation.

Ordinary CI tests the evaluator's sample, environment and correlation validation
without enforcing machine timings. The manual [performance workflow](../.github/workflows/performance.yml)
enforces thresholds only on the dedicated `rui-perf-i7-1280p` runner and retains
raw JSON even on failure. That runner must be provisioned with the pinned native
libraries, system Chromium, Xvfb and no concurrent jobs. Its remote provisioning
has not been demonstrated locally. `--enforce --runner rui-perf-i7-1280p` exits
nonzero for the currently known miss; enforcement on an unnamed runner is rejected.
Changing hardware, browser or workload requires a reviewed procedure/baseline
update and fresh three-run evidence; this evaluator refuses silent drift.
