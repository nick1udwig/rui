#!/usr/bin/env python3
"""Retain only exact bounded whole-view vector bytes, with local pixel evidence."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil


def read(path, maximum):
    if path.stat().st_size > maximum:
        raise ValueError('retained whole-view input ceiling')
    data = path.read_bytes()
    if len(data) > maximum:
        raise ValueError('retained whole-view input grew')
    return data


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('run', type=Path)
    args = parser.parse_args()
    base = Path(__file__).parent
    report = json.loads(read(args.run / 'whole-report.json', 1 << 20))
    pixels = json.loads(read(args.run / 'whole-pixels.json', 1 << 20))
    if report['failures'] or len(report['publications']) != 5 or not pixels['passed'] or len(pixels['captures']) != 5:
        raise ValueError('incomplete actual native whole-view or visual evidence')
    if report['capture_budget']['after_close'] or report['capture_budget']['peak'] > report['capture_budget']['limit']:
        raise ValueError('capture quota cleanup failure')
    staging = base / 'whole-golden.next'
    if staging.exists():
        raise ValueError('staging directory already exists')
    staging.mkdir()
    closure = set()
    for publication, comparison in zip(report['publications'], pixels['captures']):
        name = publication['label']
        manifest_bytes = read(args.run / (name + '.json'), 1 << 20)
        manifest = json.loads(manifest_bytes)
        envelope = read(args.run / (name + '.bin'), 1 << 20)
        digest = hashlib.sha256(envelope).hexdigest()
        if digest != manifest['envelope_sha256'] or digest != comparison['envelope_sha256']:
            raise ValueError('native capture / pixel comparison exact envelope mismatch')
        if comparison['capture'] != name + '.json' or comparison['local_reference_sha256'] != manifest['local_reference']['sha256']:
            raise ValueError('source local comparison mismatch')
        if not comparison['pixels']['passed'] or comparison['pixels']['interior_bad_pixels']:
            raise ValueError('visual tolerance/interior failure')
        closure.update(manifest['resources'])
        (staging / (name + '.json')).write_bytes(manifest_bytes)
        (staging / (name + '.bin')).write_bytes(envelope)
    total = 0
    for digest in sorted(closure):
        body = read(args.run / (digest + '.rvp'), 16 << 20)
        total += len(body)
        if total > 32 << 20 or not body.startswith(b'RVP1'):
            raise ValueError('bounded vector-only union closure failure')
        (staging / (digest + '.rvp')).write_bytes(body)
    hashes = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(staging.iterdir())}
    report.update(retained_exact_trace='fixtures/gtk/whole-golden', retained_trace_sha256=hashes,
        pixels=pixels, trace_scope='Five exact complete coherent source-owned whole-view publications. No raster diagnostics enter retained protocol trace.',
        interaction_scope='Native cooperating fixture lifecycle callbacks; common-ledger remote native actions are separate native-interaction.json evidence. This slice does not wire whole-view actions or Presented readiness to the transport.')
    destination = base / 'whole-golden'
    if destination.exists():
        shutil.rmtree(destination)
    staging.rename(destination)
    (base / 'whole-interaction.json').write_text(json.dumps(report, indent=2, sort_keys=True) + '\n')
    print('retained', len(hashes), 'whole-view vector files')


if __name__ == '__main__':
    main()
