#!/usr/bin/env python3
"""Repeat the frozen release CPU/TLS workload without concurrent builds."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--runs', type=int, default=3)
args = parser.parse_args()
if not 3 <= args.runs <= 10:
    parser.error('requires 3–10 runs')
args.output.mkdir(parents=True, exist_ok=False)
binary = ROOT / 'target/release/examples/measure_integrated'
if not binary.is_file():
    parser.error('build the locked release example before measuring')
sources = ('Cargo.lock', 'transport/examples/measure_integrated.rs',
           'transport/src/client.rs', 'client-core/src/replica.rs',
           'protocol-core/src/receiver.rs', 'client-desktop/src/renderer.rs',
           'host-broker/src/lib.rs', 'host-broker/src/performance.rs',
           'host-broker/src/source_fonts.rs', 'fixtures/render/native_font.py')
environment = {'kernel': platform.platform(),
               'rust': subprocess.check_output(['rustc', '--version'], text=True).strip(),
               'native': subprocess.check_output(['pkg-config', '--modversion', 'gtk4', 'cairo', 'pango'], text=True).splitlines(),
               'binarySha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
               'sourceSha256': {name: hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in sources},
               'conditions': 'release, loopback TLS, CPU frames, no concurrent builds/tests; existing 10000 records/2500 glyphs, 10 warmup and 100 measured updates per phase; fixed original ceilings'}
summaries = []
for run in range(args.runs):
    print(f'large workload run {run + 1}/{args.runs}', flush=True)
    started = time.monotonic()
    with (args.output/f'run-{run+1}.json').open('w') as output:
        subprocess.run([str(binary)], cwd=ROOT,
                       env={**os.environ, 'RUI_TIMINGS': '16384'},
                       stdout=output, check=True, timeout=1200)
    report = json.loads((args.output/f'run-{run+1}.json').read_text())
    assert not report['recovery_only']
    assert [r['changed_records'] for r in report['updates']] == [1, 20]
    assert report['records'] == 10000 and report['visible_glyph_cells'] == 2500
    assert report['resource_bodies_warm'] == report['idle_publication_bytes_250ms'] == 0
    assert report['released_replay_and_renderer_bytes'] == 0
    assert report['slow_view_closed'] != 'None'
    for key, ceiling in [('peak_host_view_bytes', 128<<20), ('peak_receiver_bytes', 128<<20),
                         ('peak_renderer_bytes', 256<<20), ('sampled_outbound_queue_bytes', 65536)]:
        assert report[key] <= ceiling, key
    for update in report['updates']:
        assert update['changed_records'] in (1, 20)
        assert len(update['raw_samples']['publication']) == 100
    assert [r['mode'] for r in report['recovery']] == ['retained_resume', 'retained_cache_loss', 'forced_fresh_scope']
    for row in report['recovery']:
        assert row['pending_commands'] == 0 and not row['presented']
        assert row['receiver_peak_bytes'] <= 128 << 20
        assert row['host_peak_bytes'] <= 128 << 20
        assert row['renderer_peak_bytes'] <= 256 << 20
    summaries.append({'run': run+1, 'wallSeconds': time.monotonic()-started,
                      'updates': [{k:v for k,v in update.items() if k != 'raw_samples'} for update in report['updates']],
                      'peakRendererBytes': report['peak_renderer_bytes'],
                      'cold': report['cold'], 'recovery': report['recovery']})
    print(json.dumps(summaries[-1]), flush=True)
(args.output/'summary.json').write_text(json.dumps({'environment': environment, 'runs': summaries,
    'sixtyHzGoal': 'Outstanding unless individual CPU render p95 is at most 16667 microseconds; this fixture does not measure native presentation.'}, indent=2)+'\n')
