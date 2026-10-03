#!/usr/bin/env python3
"""Measure native source rejection on owned displays and a service-free bus."""
import argparse
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
    root = Path(__file__).resolve().parents[2]
    run_private(args.xvfb, ['python3', str(root / 'fixtures/gtk/negative_source.py'), '--output', str(args.output)])
    report = json.loads((args.output / 'negative-report.json').read_text())
    if len(report['cases']) != 6 or report['fallback_probe']['forbidden_export_calls']:
        raise RuntimeError('missing native source cases or forbidden fallback attempt')
    print(json.dumps({'cases': [case['case'] for case in report['cases']], 'probe': report['fallback_probe']}, sort_keys=True))


if __name__ == '__main__':
    main()
