# Integrated load measurements

The [controlled performance gates](performance-gates.md) freeze the small-demo
procedure and preserve the currently unmet action targets.

The [P4 browser optimization](browser-render-performance.md) retains three-run
[before](../fixtures/performance/browser-p4-before/summary.json) and
[after](../fixtures/performance/browser-p4-after/summary.json) small-demo reports,
plus a separately measured renderer fixture. They do not supersede the native
large-workload contract or establish the remaining end-to-end targets.

## Stage instrumentation and warm browser runner (P1)

Set `RUI_TIMINGS=4096` (at most 16,384 samples) to enable numeric native/source
timings. `RUI_TIMINGS_REPORT` selects the Rust host report;
`RUI_SOURCE_TIMINGS_REPORT` selects the cooperating source report, requested on
the ordered private pipe before source disposal. Neither records packet bodies,
credentials or application text. Old samples are overwritten with a drop count.
Diagnostics have a separate bounded allocation from the negotiated wire ledger.

The browser accepts an optional `Timings` recorder and the demo enables one with
`?timings`. It records receipt/reducer work, resource waiting, committed-state
export, render preparation, composition/coverage, output conversion, submission
and the following visible animation-frame opportunity. Command results correlate
with causal presentation even when the result arrives after the frame. Lost
connections discard bounded correlation state without declaring presentation.
Native render preparation, composition and output conversion are measured
separately; GTK snapshot-to-after-paint is an actual native opportunity.

Source callback-to-capture, capture scheduling/completion, projection, encoding,
resource closure and native outgoing acknowledgement queue residence use local
monotonic clocks. Nested totals overlap: do not add capture, projection and
encoding, or receipt and replica application, as disjoint intervals. Zero-revision
queue samples concern protocol replies rather than an identifiable publication.
Queue cancellation and socket buffering remain distinct from admission timings.

Build release binaries, WASM and the production Vite bundle before running:

```sh
RUI_WASM_CHROMIUM=/path/to/chromium python3 demos/web/performance.py \
  --xvfb /path/to/Xvfb --output /tmp/rui-performance-NEW
```

The runner uses three independent runs, five warmups and 200 measured actions
per origin per run. Its bounded workload deadline is separate from the ordinary
45-second correctness harness. Raw stage samples, hardware/compiler/native and
browser versions, source hashes, viewport/network conditions, correctness results
and round-trip percentiles are retained as JSON. Harness action samples include
native-event delivery and polling and end when the causal presentation listener
updates the UI. Screenshots are outside those loops. Local spans never subtract
host and browser timestamps.

This first P1 series establishes the small-demo runner and instrumentation for
the existing large fixture. Expanded cold/reset/cache-loss, lifecycle memory,
throttled-reader and isolated steady-render reports are still required before
claiming the entire P1 completion boundary. Existing evidence below remains
historical and is not replaced by these new measurements.

The [three retained baseline runs](../fixtures/performance/browser-p1/summary.json)
contain 200 warm browser and 200 warm source actions each on the recorded
i7-1280P, Chromium 151, GTK 4.22.4/Cairo 1.18.4 and Rust 1.98.1 environment.
Browser harness p95 ranges from 173.3 to 185.7 ms; source harness p95 ranges from
296.3 to 302.2 ms. The source measurements include the pre-P2 250 ms polling
schedule. Neither origin meets the future small-demo gate under this procedure.
Raw reports include causal command timing and stage samples; they make no
physical scanout or general 60 Hz claim.

The [P2 comparison](../fixtures/performance/browser-p2/summary.json) repeats the
same procedure after notification-driven capture. Source harness p95 is
153.4–154.2 ms and p99 is 154.9–157.0 ms. Browser harness p95 varies from
133.0–186.2 ms; this does not establish a consistent browser-origin speedup.
Native-event delivery and 50 ms harness polling remain in these round trips.
The scheduling delay is removed but the future 100 ms target remains unmet.


The [retained report](../fixtures/performance/integrated.json) measures the actual TLS publication/resource path on an Intel Core i7-1280P, with Rust 1.98.1 and the pinned Pango/Cairo outline provider.
It records native versions, source/font hashes, ceilings and measurement conditions.
Other compiler/native test work ran concurrently; these are observed development-machine timings.

The source has 10,000 meaningful records: a Session and Surface, 2,500 visible glyph cells, 2,499 spatial scene groups, and corresponding semantic leaves/groups.
The 1,000×600 canvas shares one immutable native glyph resource.
One-record updates change Surface opacity/capture evidence; twenty-record updates also change nineteen glyph colors.
Each phase has ten warmup updates followed by 100 measured updates.
Two healthy TLS clients consume and render every update; a third observer stops reading after its initial checkpoint and closes at its bounded queue limit.

| Measured interval, milliseconds (p50 / p95 / p99) | One record | Twenty records |
|---|---|---|
| Capture/publication across connected views | 17.672 / 21.146 / 25.046 | 11.867 / 14.103 / 16.121 |
| Source change through two clients applying and emitting ACKs | 33.351 / 42.102 / 48.723 | 27.201 / 32.894 / 38.079 |
| Two CPU renders and pixel comparison | 1536.423 / 1741.703 / 1862.728 | 1491.566 / 1584.989 / 1779.112 |
| Transaction encoding | 0.002 / 0.003 / 0.004 | 0.007 / 0.009 / 0.010 |
| Separate replica application | 7.958 / 9.701 / 12.249 | 7.827 / 9.633 / 10.184 |

The stalled observer closes during the first phase, so the phases have different publication fanout.
Their timings do not isolate the cost of changing more records.
The separate replica uses the Rust reducer on the exact recorded transaction payload; independent-language interoperability is checked elsewhere.
The encoding interval re-encodes the decoded recorded transaction; original publication encoding is included in capture/publication.
ACK timing ends after client socket flush, without a claim that the host has processed those ACKs.

The initial checkpoint is 992,674 bytes.
Updates are 237 bytes for one record and 4,323–4,326 bytes for twenty.
The cold resource body is delivered once; warm updates send no resource bodies (100% reuse).
One client's complete recorded plaintext traffic totals 1,587,035 bytes.
A 250 ms idle interval produces no publication traffic.
Each update checks equal receiver records and pixels; the final state also equals the direct source capture and its full-canvas render.
The separate [native gate](../client-desktop/tests/performance_source.rs) checks visible pixels in all 2,500 cells and exact one/twenty-record changes.

| Ownership metric | Observed bytes | Ceiling bytes |
|---|---:|---:|
| Host view lifetime high-water | 119,301,824 | 134,217,728 |
| Receiver lifetime high-water | 62,318,600 | 134,217,728 |
| Renderer lifetime high-water | 268,124,192 | 268,435,456 |
| Sampled outbound queue | 63,978 | 65,536 |

Allocation counters measure conservative reservations including transient peaks; queue measurements are samples.
Whole-process peak RSS is separately reported as 129,132 kB.
Renderer headroom is only 311,264 bytes for this fixed workload.
Replay and renderer reservations return to zero, and all scoped diagnostic owners close.

This CPU renderer misses the provisional 16.67 ms opportunity at 60 Hz.
These measurements satisfy the declared PER-001 load checks.
The separate actual native measurements below do not infer presentation from CPU rendering or Applied.

Reproduce on a machine with the pinned native font dependencies:

```sh
cargo run --offline --release -p rui-transport --example measure_integrated
cargo test --offline -p rui-client-desktop --test performance_source -- --ignored
```

The benchmark requires loopback sockets.
It uses fixture TLS credentials and an explicit local recorder; no desktop session is required.
The JSON emitted by the executable contains measurements; the retained report additionally records the build environment and source hashes.

## Actual native input through after-paint

The root-reviewed [native broad report](../fixtures/render/native-broad.json) measures fourteen successful source actions in a debug build on the same i7-1280P, GTK 4.22.4, Linux 7.1.9.
Each sample starts inside the actual GTK input callback before enqueue and ends at the actual after-paint callback for its causal revision.
Canvas timing starts at release, when its complete stroke is submitted.
The controller crosses a 32 KiB/s relay; the observer connects directly.
Both mirrors, the controlled source and the diagnostic comparisons run on this machine.
Other compiler/native test work ran concurrently.

| Actual callback latency, milliseconds | p50 | p95 | p99 |
|---|---:|---:|---:|
| Controller | 1971.926 | 2386.027 | 2386.027 |
| Observer | 1675.626 | 1931.399 | 1931.399 |

The small, heterogeneous sample includes checkbox, selection/reorder, scroll, transients, canvas and source resize, following initial resources, typing and viewport warmup.
With fourteen samples, p95 and p99 are the maximum observation.
All fourteen actions succeeded without timeout; deliberate pre-submission overflow and Escape cancellations are reported separately.
Both clients exceeded their reported 16.667 ms interval on every sample.
CPU render/RGBA/diagnostic work also exceeded the configured opportunity in all 30 controller and 31 observer attempts.
The report retains individual samples and worker scheduling counters.
Elapsed timer periods are not a count of physically dropped display frames.

These timings are measured limitations, not a 60 Hz performance pass.
After-paint on the owned Xvfb is a native callback boundary and makes no physical scanout claim.

The [release-build run](../fixtures/performance/native-release.json) repeats the same fourteen-action scenario from the frozen formatted source.
Its controller p50/p95/p99 is **762.356 / 1024.619 / 1024.619 ms**; the direct observer measures **214.932 / 381.116 / 381.116 ms**.
All fourteen samples still exceed the reported frame interval.
Maximum render/RGBA/diagnostic work is 204.045 ms for the controller and 249.144 ms for the observer, with every attempted frame over budget.

The transport regression briefly overlapped this run's start, and targeted agent work may also have run.
The retained report includes those conditions, source fingerprints, individual samples, exact trace hashes, and root independent audits of all 54 publication prefixes and 150 vector resource transfers.
The earlier debug trace is retained for deterministic CI replay.
Reproduce release timing by building `corpus_smoke` with `--release` and passing `--target-dir target/release` to `fixtures/render/native_smoke.py`.
