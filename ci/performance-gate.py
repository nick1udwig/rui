#!/usr/bin/env python3
"""Frozen P5 small-demo gates; enforcement is for the named dedicated runner."""
import argparse
import json
import math
from pathlib import Path

RUNNER = 'rui-perf-i7-1280p'
ENVIRONMENT = {'hardware': '12th Gen Intel(R) Core(TM) i7-1280P',
               'browser': '151.0.7922.173', 'rust': 'rustc 1.98.1 (48a229cea 2026-09-01)',
               'native': ['4.22.4', '1.18.4'], 'viewport': {'width': 1200, 'height': 1200, 'scale': 0.75},
               'network': 'loopback TLS with certificate-verifying proxy', 'build': 'release'}


def samples(values):
    if len(values) < 200 or len(values) > 16384 or any(type(v) not in (int, float) or not math.isfinite(v) or not 0 <= v <= 300_000 for v in values):
        raise ValueError('bounded numeric sample population required')
    ordered = sorted(values)
    return {f'p{p}': ordered[math.ceil(len(ordered)*p/100)-1] for p in (50, 95, 99)}


def draw_samples(timing):
    result, frame = [], None
    stages = {'preparation', 'composition', 'outputConversion', 'submission'}
    for row in timing['samples']:
        if row['stage'] == 'preparation':
            frame = {'revision': row['revision'], 'values': {}}
        if frame is not None and row['stage'] in stages:
            if row['revision'] != frame['revision'] or row['stage'] in frame['values']:
                raise ValueError('invalid render-stage correlation')
            frame['values'][row['stage']] = row['durationMs']
            if row['stage'] == 'submission':
                if set(frame['values']) != stages:
                    raise ValueError('incomplete render interval')
                result.append(sum(frame['values'].values()))
                frame = None
    return result


def evaluate(reports):
    if len(reports) != 3:
        raise ValueError('exactly three runs required by the frozen procedure')
    rows = []
    for report in reports:
        if report['schemaVersion'] != 1 or any(report.get(k) != v for k, v in ENVIRONMENT.items()):
            raise ValueError('runner/browser/native/build/viewport profile mismatch')
        if not report['correctness']['passed']:
            raise ValueError('correctness companion failed')
        timing = report['timing']
        if timing['dropped'] or len(timing['samples']) > timing['capacity'] or timing['capacity'] > 16384:
            raise ValueError('timing ring dropped required evidence')
        row = {}
        for origin in ('browser', 'source'):
            actions = [s['durationMs'] for s in report['actionSamples'] if s['kind'] == origin]
            if len(actions) != 200:
                raise ValueError('exactly 200 measured actions per origin required')
            distribution = samples(actions)
            row[origin] = {**distribution, 'passed': distribution['p95'] <= 100 and distribution['p99'] <= 150}
        # Frozen runner has 400 measured causal frames after the correctness
        # companion and five warmups per origin. Rendering is synchronous;
        # sum stages within a frame, never sum independently ranked percentiles.
        draw = samples(draw_samples(timing)[-400:])
        row['draw'] = {**draw, 'passed': draw['p95'] <= 16.67}
        rows.append(row)
    return {'runner': RUNNER, 'runs': rows,
            'passed': all(gate['passed'] for row in rows for gate in row.values()),
            'scope': 'small GTK loopback demo; action samples include harness polling/native injection and end at causal presentation; draw is separate',
            'large60Hz': 'outstanding: native large fixture still exceeds 16.67 ms',
            'expandedFaultMeasurements': 'Evaluated separately in docs/browser-lifecycle-performance.md and docs/large-performance.md; this small-demo evaluator adds no thresholds for those scenarios'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--enforce', action='store_true')
    parser.add_argument('--runner')
    args = parser.parse_args()
    if args.enforce and args.runner != RUNNER:
        parser.error('timing enforcement requires the named dedicated benchmark runner')
    reports = []
    for number in range(1, 4):
        path = args.directory / f'run-{number}.json'
        if path.stat().st_size > 8 << 20:
            raise ValueError('report byte ceiling')
        reports.append(json.loads(path.read_text()))
    result = evaluate(reports)
    raw = json.dumps(result, indent=2)+'\n'
    if args.output:
        with args.output.open('x') as stream:
            stream.write(raw)
    print(raw, end='')
    if args.enforce and not result['passed']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
