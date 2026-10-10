#!/usr/bin/env python3
"""Measure warm source/browser actions on owned displays, with a workload deadline."""
import argparse
import json
import hashlib
import math
import os
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from fixtures.gtk.mirror_check import run_private


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--xvfb', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--actions', type=int, default=200)
    parser.add_argument('--runs', type=int, default=3)
    args = parser.parse_args()
    if not 200 <= args.actions <= 1000 or not 3 <= args.runs <= 10:
        parser.error('requires 200–1000 actions and 3–10 runs')
    args.output.mkdir(parents=True, exist_ok=False)
    os.environ.update(RUI_PERFORMANCE_ACTIONS=str(args.actions), RUI_BROWSER_BUILD='1', RUI_BROWSER_HTTPS='1')
    summary = []
    for run in range(args.runs):
        # Builds must finish before this command. Each run includes a cold
        # correctness companion, five warmups, then the measured action loops.
        stdout = run_private(str(args.xvfb.resolve()), ['node', str(ROOT/'demos/web/check.mjs')], timeout=min(1800, 120+args.actions*3))
        report = json.loads((ROOT/'client-browser/test-output/timings.json').read_text())
        report['correctness'] = json.loads(stdout.strip().splitlines()[-1])
        report['sourceHashes'] = {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in [ROOT/n for n in ('adapters/gtk4/native.py', 'adapters/sdk/producer.py', 'adapters/sdk/timing.py', 'adapters/sdk/src/gtk_host.rs', 'adapters/sdk/src/gtk_port.rs', 'adapters/sdk/src/source_notify.rs', 'adapters/sdk/invalidation.py', 'client-desktop/src/bin/rui-demo.rs', 'protocol-core/src/timing.rs', 'client-browser/src/client.ts', 'client-browser/src/renderer.ts', 'client-browser/src/timing.ts', 'demos/web/check.mjs')]}
        (args.output/f'run-{run+1}.json').write_text(json.dumps(report)+'\n')
        row = {'run': run+1}
        for kind in ('browser', 'source'):
            samples = sorted(s['durationMs'] for s in report['actionSamples'] if s['kind'] == kind)
            assert len(samples) == args.actions
            row[kind] = {f'p{p}': samples[math.ceil(len(samples)*p/100)-1] for p in (50,95,99)}
        summary.append(row)
        print(json.dumps(row), flush=True)
    (args.output/'summary.json').write_text(json.dumps({'runs': summary, 'conditions': 'loopback release; polling included in harness round trips; local process spans use monotonic clocks; screenshot excluded; no physical scanout claim'})+'\n')


if __name__ == '__main__':
    main()
