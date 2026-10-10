#!/usr/bin/env python3
"""Bounded aggregation of actual submitted-tree coverage, including negatives."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import platform

ROOT = Path(__file__).resolve().parents[2]


def inventory(measurement, negatives):
    rows = []
    if measurement['pin'] != {'gtk': '4.22.4', 'cairo': '1.18.4'}:
        raise ValueError('coverage native pin mismatch')
    for run in measurement['runs']:
        frames = run['captures']
        if not 1 <= len(frames) <= 64:
            raise ValueError('bounded submitted frames required')
        nodes, gaps = Counter(), Counter()
        complete, partial = 0, 0
        for frame in frames:
            lifecycle = (frame['capture_boundary'] == 'source-lifecycle-signal'
                         and not frame['mapped'] and not frame['node_counts'] and not frame['gaps'])
            if frame['capture_boundary'] != 'actual-snapshot-vfunc-same-node-append' and not lifecycle:
                raise ValueError('sample is not an actual submitted tree')
            if sum(frame['node_counts'].values()) > 4096 or len(frame['gaps']) > 4096:
                raise ValueError('source node ceiling')
            nodes.update(frame['node_counts'])
            gaps.update(gap['reason'] for gap in frame['gaps'])
            if frame['mapped'] and not frame.get('destroyed'):
                partial += bool(frame['gaps'])
                complete += not frame['gaps']
        rows.append({'style': run['theme'], 'frames': len(frames),
                     'mappedComplete': complete, 'mappedPartial': partial,
                     'submittedNodeCounts': dict(sorted(nodes.items())),
                     'gapOccurrences': sum(gaps.values()), 'uniqueGapReasons': len(gaps),
                     'gapReasons': dict(sorted(gaps.items())),
                     'surfaces': sorted({f['surface'] for f in frames}),
                     'actions': sorted({a['name'] for a in run['actions']})})
    cases = {case['case']: bool(case['gaps']) for case in negatives['cases']}
    required = {'original-photo', 'memory-texture', 'cairo-region', 'blur-effect', 'shader-node'}
    if not required <= cases.keys() or not all(cases[name] for name in required):
        raise ValueError('unsupported source content was silently accepted')
    if cases.get('native-vector-control', True) or negatives['fallback_probe']['forbidden_export_calls']:
        raise ValueError('vector control or no-raster boundary failed')
    return {'schemaVersion': 1, 'pin': measurement['pin'], 'backend': measurement['display_harness'],
            'runs': rows, 'negativeCases': cases,
            'scope': 'submitted cooperating content and registered popup; no decorations or desktop association',
            'countsAre': 'repeated frame occurrences, not unique controls or supported workflows'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('measurement', type=Path)
    parser.add_argument('negatives', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    inputs = [args.measurement, args.negatives]
    if any(p.stat().st_size > 8 << 20 for p in inputs):
        raise ValueError('coverage report ceiling')
    report = inventory(*(json.loads(p.read_text()) for p in inputs))
    report['environment'] = {'python': platform.python_version(), 'kernel': platform.release(), 'locale': 'C.UTF-8', 'scale': 1}
    report['inputHashes'] = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in inputs}
    report['sourceHashes'] = {p: hashlib.sha256((ROOT/p).read_bytes()).hexdigest() for p in
                             ('adapters/gtk4/capture.py', 'fixtures/gtk/corpus.py', 'fixtures/gtk/negative_source.py', 'fixtures/gtk/inventory.py')}
    with args.output.open('x') as output:
        json.dump(report, output, indent=2, sort_keys=True)
        output.write('\n')
    print(json.dumps({'styles': len(report['runs']), 'negativeCases': len(report['negativeCases'])}))


if __name__ == '__main__':
    main()
