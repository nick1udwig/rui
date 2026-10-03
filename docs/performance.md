# Integrated load measurements

The [retained report](../fixtures/performance/integrated.json) measures the actual
TLS publication/resource path on an Intel Core i7-1280P, with Rust 1.98.1 and
the pinned Pango/Cairo outline provider. It records native versions, source/font
hashes, ceilings and measurement conditions. Other compiler/native test work ran
concurrently; these are observed development-machine timings.

The source has 10,000 meaningful records: a Session and Surface, 2,500 visible
glyph cells, 2,499 spatial scene groups, and corresponding semantic leaves/groups.
The 1,000×600 canvas shares one immutable native glyph resource. One-record updates
change Surface opacity/capture evidence; twenty-record updates also change nineteen
glyph colors. Each phase has ten warmup updates followed by 100 measured updates.
Two healthy TLS clients consume and render every update; a third observer stops
reading after its initial checkpoint and closes at its bounded queue limit.

| Measured interval, milliseconds (p50 / p95 / p99) | One record | Twenty records |
|---|---|---|
| Capture/publication across connected views | 17.672 / 21.146 / 25.046 | 11.867 / 14.103 / 16.121 |
| Source change through two Applied acknowledgments | 33.351 / 42.102 / 48.723 | 27.201 / 32.894 / 38.079 |
| Two CPU renders and pixel comparison | 1536.423 / 1741.703 / 1862.728 | 1491.566 / 1584.989 / 1779.112 |
| Transaction encoding | 0.002 / 0.003 / 0.004 | 0.007 / 0.009 / 0.010 |
| Separate replica application | 7.958 / 9.701 / 12.249 | 7.827 / 9.633 / 10.184 |

The stalled observer closes during the first phase, so the phases have different
publication fanout. Their timings do not isolate the cost of changing more records.
The separate replica uses the Rust reducer on the exact recorded transaction
payload; independent-language interoperability is checked elsewhere.

The initial checkpoint is 992,674 bytes. Updates are 237 bytes for one record and
4,323–4,326 bytes for twenty. The cold resource body is delivered once; warm updates
send no resource bodies (100% reuse). One client's complete recorded plaintext
traffic totals 1,587,035 bytes. A 250 ms idle interval produces no publication
traffic. Each update checks equal receiver records and pixels; the final state
also equals the direct source capture and its full-canvas render. The separate
[native gate](../client-desktop/tests/performance_source.rs) checks visible pixels
in all 2,500 cells and exact one/twenty-record changes.

| Ownership metric | Observed bytes | Ceiling bytes |
|---|---:|---:|
| Host view lifetime high-water | 119,301,824 | 134,217,728 |
| Receiver lifetime high-water | 62,318,600 | 134,217,728 |
| Renderer lifetime high-water | 268,124,192 | 268,435,456 |
| Sampled outbound queue | 63,978 | 65,536 |

Allocation counters measure conservative reservations including transient peaks;
queue measurements are samples. Whole-process peak RSS is separately reported as
129,132 kB. Renderer headroom is only 311,264 bytes for this fixed workload. Replay
and renderer reservations return to zero, and all scoped diagnostic owners close.

This CPU renderer misses the provisional 16.67 ms opportunity at 60 Hz. These
measurements satisfy the declared PER-001 load checks; native action-to-Presented
latency and actual desktop frame opportunities remain separate M5 integration
work. No native presentation is inferred from CPU rendering or Applied.

Reproduce on a machine with the pinned native font dependencies:

```sh
cargo run --offline --release -p rui-transport --example measure_integrated
cargo test --offline -p rui-client-desktop --test performance_source -- --ignored
```

The benchmark requires loopback sockets. It uses fixture TLS credentials and
an explicit local recorder; no desktop session is required. The JSON emitted by
the executable contains measurements; the retained report additionally records
the build environment and source hashes.
