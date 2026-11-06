# Bandwidth strategy evidence

These offline results support [Directions for reducing RUI bandwidth](../../../../docs/rui-bandwidth-directions.md).
The input is the existing [paired game comparison](../comparison/README.md), with 200 samples per game at 25 Hz.
No new game capture is required.

`summary.json` records source hashes, codec versions, settings, per-frame sizes, compression-call timing, and reconstruction checks.
`tables.md` is generated from that summary by [strategies.py](../../bench/strategies.py).
Transport probes preserve the complete original core envelope bytes.
Beam probes preserve the exact source floating-point bits, then verify byte-identical re-export through the actual RUI game projector.
Beam sizes exclude protocol records, source control metadata, and semantic controls.
Every update is flushed without waiting for later updates.
Zstandard uses a maximum 256 KiB history window; total codec memory is not measured.
A compression reset is not a scene recovery checkpoint.

Reproduce into a new output directory:

```sh
cargo build --release --locked -p rui-browser-host --bin game-trace
python3 demos/games/bench/strategies.py --output /tmp/rui-strategies --game-trace target/release/game-trace
```

The probes require Python 3.14 or newer with `compression.zstd`.
Three repetitions check deterministic byte counts; timing varies by machine and load.
The timing covers codec calls and context creation, not beam packing, rendering, network transfer, or browser decoding.
These results do not change the production protocol or demonstrate end-to-end phone performance.
