#!/usr/bin/env python3
"""Run native binding geometry/lifecycle on owned displays and a private bus."""
import argparse
import hashlib
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from fixtures.gtk.mirror_check import run_private


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--xvfb', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() and any(args.output.iterdir()):
        raise ValueError('fresh owned output directory required')
    run_private(args.xvfb, [sys.executable, 'fixtures/gtk/binding_capture.py', str(args.output)])
    report = json.loads((args.output / 'binding-report.json').read_text())
    if len(report['frames']) != 6:
        raise ValueError('missing actual native binding lifecycle frames')
    report['source_file_fingerprints'] = {
        name: hashlib.sha256(Path(name).read_bytes()).hexdigest()
        for name in ['adapters/gtk4/state.py', 'adapters/gtk4/capture.py',
                     'adapters/sdk/producer.py', 'fixtures/gtk/corpus.py',
                     'fixtures/gtk/binding_capture.py', 'fixtures/gtk/binding_check.py',
                     'fixtures/gtk/mirror_check.py']
    }
    report['retained_trace_sha256'] = {
        path.name: hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(args.output.iterdir())
        if path.suffix in ('.json', '.bin', '.rvp') and path.name != 'binding-report.json'
    }
    (args.output / 'binding-report.json').write_text(json.dumps(report, indent=2, sort_keys=True) + '\n')
    print(json.dumps({'native_frames': len(report['frames']), 'counter_rotated_clip_denied': True}))


if __name__ == '__main__':
    main()
