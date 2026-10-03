#!/usr/bin/env python3
"""Freeze one completed isolated native run's exact vector trace and hashes.

Local reference images and native failure receipts are deliberately excluded
from the protocol trace. Run this only on the controlled fixture output.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('run', type=Path)
    args = parser.parse_args()
    target = Path(__file__).parent
    trace = args.run / 'trace'
    report_path = args.run / 'interaction.json'
    if report_path.stat().st_size > 32 << 20:
        raise SystemExit('interaction report ceiling')
    report = json.loads(report_path.read_text())
    files = list(trace.rglob('*'))
    bodies = [path for path in files if path.is_file()]
    if sum(path.stat().st_size for path in bodies) > 64 << 20 or any(path.suffix not in ('.json', '.bin', '.rvp') for path in bodies):
        raise SystemExit('finite exact vector trace ceiling or unexpected file')
    staging = target / 'native-golden.next'
    if staging.exists():
        raise SystemExit('staging directory already exists')
    shutil.copytree(trace, staging)
    hashes = {str(path.relative_to(staging)): hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(staging.rglob('*')) if path.is_file()}
    report.update(pinned_gtk=[4, 22, 4], retained_exact_trace='fixtures/gtk/native-golden',
                  retained_trace_sha256=hashes,
                  observer_scope='Independent consumer instances of the same Rust reducer; independent Python implementation interop is separate evidence.',
                  exporter_audit_scope='Exact source geometry, closure/descriptors and named coverage gaps audited against decoded Rust state. Schema2 observations and source extraction origins retained for every publication; exporter Python pixel/draw/serialize traps active throughout native fixture. Native GTK local drawing is outside the probe boundary.')
    destination = target / 'native-golden'
    shutil.rmtree(destination)
    staging.rename(destination)
    (target / 'native-interaction.json').write_text(json.dumps(report, indent=2, sort_keys=True) + '\n')
    print(f'retained {len(hashes)} exact vector files')


if __name__ == '__main__':
    main()
