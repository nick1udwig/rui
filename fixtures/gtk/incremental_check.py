#!/usr/bin/env python3
"""Independent full-record replay of authorized incremental GTK evidence."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from conformance.full_reference import FullReplica
from conformance.reference import fields, one, number, many


def check(root, report):
    inventory = report['files']
    if len(inventory) > 256 or sum((root / name).stat().st_size for name in inventory) > 8 << 20:
        raise ValueError('evidence ceiling')
    for name, digest in inventory.items():
        path = root / name
        if path.parent != root or path.is_symlink() or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError('evidence identity')
    oracle = FullReplica()
    requests, expected, kinds = [], [], []
    scope = None
    for path in sorted(root.glob('public-*.bin')):
        envelope = fields(path.read_bytes())
        binding = tuple(one(envelope, tag, 2) for tag in (2, 3, 4))
        if any(len(value) != 16 for value in binding) or scope is not None and scope != binding:
            raise ValueError('scope mismatch')
        scope = binding
        kind = {1: 'snapshot', 2: 'transaction'}[number(envelope, 5)]
        payload = one(envelope, 6, 2)
        resource_tag = 3 if kind == 'snapshot' else 4
        if many(fields(payload), resource_tag, 2):
            raise ValueError('this corpus declares semantic-only without resources')
        state = oracle.apply(kind, payload)
        committed = json.loads(path.with_suffix('.state.json').read_text())
        if state != committed:
            raise ValueError('independent full-state mismatch')
        requests.append({'kind': kind, 'hex': payload.hex(), 'available': []})
        expected.append(committed)
        kinds.append(kind)
    run = subprocess.run([str(ROOT/'target/debug/examples/replay'), '--full-state'],
                         input=''.join(json.dumps(r)+'\n' for r in requests),
                         text=True, capture_output=True, check=True, timeout=30)
    actual = [json.loads(line) for line in run.stdout.splitlines()]
    if len(actual) != len(expected) or any(not a['accepted'] or a['state'] != e for a, e in zip(actual, expected)):
        raise ValueError('Rust/independent replay mismatch')
    if kinds.count('transaction') < 3 or not oracle.retired:
        raise ValueError('missing incremental/retirement evidence')
    return {'prefixes': len(expected), 'snapshots': kinds.count('snapshot'),
            'transactions': kinds.count('transaction'), 'retired': len(oracle.retired),
            'finalRevision': oracle.revision, 'allFieldsAgree': True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT/'fixtures/gtk/incremental-golden')
    parser.add_argument('--report', type=Path, default=ROOT/'fixtures/gtk/incremental-publications.json')
    args = parser.parse_args()
    print(json.dumps(check(args.root, json.loads(args.report.read_text()))))


if __name__ == '__main__':
    main()
