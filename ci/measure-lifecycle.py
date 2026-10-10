#!/usr/bin/env python3
"""Repeat installed-package lifecycle measurements on private owned displays."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from fixtures.gtk.mirror_check import run_private


def distribution(values):
    assert values and all(math.isfinite(v) and 0 <= v <= 30000 for v in values)
    ordered = sorted(values)
    return {f'p{p}': ordered[math.ceil(len(values)*p/100)-1] for p in (50, 95, 99)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--xvfb', type=Path, required=True)
    parser.add_argument('--runs', type=int, default=3)
    args = parser.parse_args()
    if not 3 <= args.runs <= 10:
        parser.error('requires 3–10 runs')
    args.output.mkdir(parents=True, exist_ok=False)
    os.environ.update(RUI_LIFECYCLE='1', RUI_BROWSER_HEADED='1')
    sources = ('Cargo.lock', 'client-browser/scripts/api-baseline.json',
               'client-browser/src/client.ts', 'client-browser/src/renderer.ts',
               'client-browser/tests/consumer/main.ts',
               'client-browser/tests/package-live.mjs',
               'client-browser/tests/lifecycle.mjs', 'client-core/src/replica.rs',
               'adapters/sdk/src/gtk_host.rs', 'adapters/sdk/src/gtk_port.rs')
    hashes = {name: hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in sources}
    artifacts = ('target/release/rui-demo', 'client-browser/wasm/rui_client_wasm_bg.wasm')
    hashes.update({name: hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in artifacts})
    rows = []
    for number in range(1, args.runs+1):
        print(f'lifecycle run {number}/{args.runs}', flush=True)
        # Package installation/typecheck/build precede host startup and all
        # measured intervals. Do not run unrelated builds/tests concurrently.
        stdout = run_private(str(args.xvfb.resolve()),
                             ['node', str(ROOT/'client-browser/tests/package-live.mjs')],
                             timeout=180)
        assert json.loads(stdout.strip().splitlines()[-1])['passed']
        report = json.loads((ROOT/'client-browser/test-output/lifecycle.json').read_text())
        report['sourceHashes'] = hashes
        assert report['disposal'] == {'rust': 0, 'renderer': 0}
        assert report['hidden']['controlReleased']
        assert report['hidden']['verifiedRefillWithoutPresentation']
        assert not report['hidden']['authorityRestored']
        assert len(report['rows']['lifecycle']) == 25
        for row in report['rows']['lifecycle']:
            assert row['disposedRust'] == row['disposedRenderer'] == 0
        (args.output/f'run-{number}.json').write_text(json.dumps(report)+'\n')
        rows.append({'run': number,
                     **{kind: distribution([r['durationMs'] for r in report['rows'][kind]])
                        for kind in ('cold', 'reconnect', 'cacheLoss')},
                     'lifecycleConnection': distribution([r['reconnect']['durationMs'] for r in report['rows']['lifecycle']]),
                     'pendingOutcomeRecoveryMs': report['rows']['pendingOutcome']['recovery']['durationMs'],
                     'firstPostGcHeap': report['heaps'][0], 'lastPostGcHeap': report['heaps'][-1]})
        print(json.dumps(rows[-1]), flush=True)
    (args.output/'summary.json').write_text(json.dumps({'runs': rows,
        'conditions': 'named environment in each raw report; browser monotonic intervals through actual visible frame; one initial cold, three reconnects/cache-loss, one pending-result recovery, 25 lifecycle cycles per run; sparse cold samples are observations, not 200-action warm latency claims'}, indent=2)+'\n')


if __name__ == '__main__':
    main()
