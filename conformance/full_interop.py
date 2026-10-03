#!/usr/bin/env python3
"""Compare every retained record field in hash-verified native capture prefixes."""
import argparse
from collections import defaultdict
import hashlib
import json
from pathlib import Path
import struct
import subprocess

from .full_reference import FullReplica
from .reference import fields, one, many, number

ROOT = Path(__file__).resolve().parent.parent
CORPORA = ('gtk/golden', 'gtk/native-golden', 'gtk/unicode-golden',
           'gtk/restart-golden', 'gtk/negative-golden', 'gtk/whole-golden', 'atspi/golden')


def verify_resource(body, kind, version, provenance, digest):
    if (kind, version, provenance) not in ((1, 1, 1), (2, 1, 2)) or len(body) > 4 << 20:
        raise ValueError('unsupported bounded vector resource')
    domain = b'RUI/0.1/resource\0' + struct.pack('>IIQI', kind, version, len(body), provenance)
    if hashlib.sha256(domain + body).hexdigest() != digest:
        raise ValueError('resource descriptor/body hash mismatch')
    if len(body) < 8 or body[:4] != b'RVP1':
        raise ValueError('non-vector resource body')
    count = struct.unpack_from('>I', body, 4)[0]
    if count > 65536:
        raise ValueError('vector command ceiling')
    offset, opened = 8, False
    for _ in range(count):
        if offset >= len(body):
            raise ValueError('truncated vector command')
        opcode = body[offset]
        offset += 1
        if opcode not in (1, 2, 3, 4, 5) or opcode != 1 and not opened:
            raise ValueError('invalid vector contour')
        opened = opcode != 5
        coordinates = {1: 2, 2: 2, 3: 4, 4: 6, 5: 0}[opcode]
        end = offset + coordinates * 8
        if end > len(body):
            raise ValueError('truncated vector coordinate')
        for (value,) in struct.iter_unpack('>d', body[offset:end]):
            if not -1e9 <= value <= 1e9 or value == 0 and struct.pack('>d', value)[0] & 128:
                raise ValueError('invalid vector coordinate')
        offset = end
    if offset != len(body):
        raise ValueError('trailing vector payload')


def captures(corpora=CORPORA):
    grouped = defaultdict(list)
    count = 0
    for corpus in corpora:
        root = ROOT / 'fixtures' / corpus
        for path in sorted(root.rglob('*.bin')):
            if path.stat().st_size > 4 << 20:
                raise ValueError('capture byte ceiling')
            raw = path.read_bytes()
            envelope = fields(raw)
            scope = tuple(one(envelope, tag, 2) for tag in (2, 3, 4))
            if any(len(identity) != 16 for identity in scope) or number(envelope, 1) != 1:
                raise ValueError('capture binding')
            kind = {1: 'snapshot', 2: 'transaction'}[number(envelope, 5)]
            payload = one(envelope, 6, 2)
            message = fields(payload)
            resources = [digest.hex() for digest in many(message, 3 if kind == 'snapshot' else 4, 2)]
            if corpus == 'atspi/golden':
                manifest = json.loads((root / 'report.json').read_text())
                assert manifest['files'][path.name] == hashlib.sha256(raw).hexdigest()
                assert not resources
            else:
                if path.with_suffix('.json').exists():
                    manifest = json.loads(path.with_suffix('.json').read_text())
                    assert manifest['envelope_sha256'] == hashlib.sha256(raw).hexdigest(), str(path)
                elif corpus == 'gtk/restart-golden':
                    manifest = json.loads((ROOT / 'fixtures/gtk/restart-interaction.json').read_text())
                    assert manifest['retained_trace_sha256'][str(path.relative_to(root))] == hashlib.sha256(raw).hexdigest()
                else:
                    raise ValueError('missing capture manifest')
                offers = {offer['digest']: offer for offer in manifest.get('resource_offers', [])}
                for digest in resources:
                    resource = path.parent / (digest + '.rvp')
                    if resource.stat().st_size > 4 << 20:
                        raise ValueError('resource body ceiling')
                    body = resource.read_bytes()
                    if digest in offers:
                        offer = offers[digest]
                        if len(body) != offer['total_bytes']:
                            raise ValueError('resource body length')
                        verify_resource(body, offer['kind'], offer['version'], offer['provenance'], digest)
                    else:
                        # Broker-public restart envelopes share the exact
                        # retained private vector bodies. The digest commits
                        # the descriptor, so only its matching approved form
                        # can be marked available in this replay harness.
                        valid = False
                        for resource_kind in (1, 2):
                            try:
                                verify_resource(body, resource_kind, 1, resource_kind, digest)
                                valid = True
                                break
                            except ValueError:
                                pass
                        if not valid:
                            raise ValueError('no approved vector descriptor matches body')
            grouped[(corpus, scope)].append({'kind': kind, 'hex': payload.hex(), 'available': resources,
                                            'revision': number(message, 1 if kind == 'snapshot' else 2),
                                            'file': str(path.relative_to(ROOT))})
            count += 1
            if count > 512:
                raise ValueError('capture count ceiling')
    return [sorted(group, key=lambda entry: entry['revision']) for group in grouped.values()]


def compare(replay, publications):
    if not publications:
        raise ValueError('empty transcript')
    requests = ''.join(json.dumps({key: item[key] for key in ('kind', 'hex', 'available')}) + '\n' for item in publications)
    result = subprocess.run([str(replay), '--full-state'], input=requests, text=True, capture_output=True, check=True, timeout=60)
    answers = result.stdout.splitlines()
    if len(answers) != len(publications):
        raise ValueError('Rust prefix count differs')
    replica = FullReplica()
    primitives, record_kinds = set(), set()
    for item, answer in zip(publications, answers):
        expected = replica.apply(item['kind'], bytes.fromhex(item['hex']))
        actual = json.loads(answer)
        if not actual['accepted']:
            raise ValueError('Rust rejected ' + item.get('file', 'network capture'))
        if actual['state'] != expected:
            mismatches = [identity for identity in expected['records']
                          if expected['records'][identity] != actual['state']['records'].get(identity)]
            raise ValueError(f"independent retained state differs at {item.get('file', 'network capture')}: {mismatches[:8]}")
        for wrapper in expected['records'].values():
            kind, record = next(iter(wrapper.items()))
            record_kinds.add(kind)
            if kind == 'Scene':
                value = record['primitive']
                primitives.add(value if isinstance(value, str) else next(iter(value)))
    return len(publications), record_kinds, primitives


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--replay', type=Path, default=ROOT / 'target/debug/examples/replay')
    args = parser.parse_args()
    prefixes, kinds, primitives = 0, set(), set()
    groups = captures()
    for group in groups:
        count, records, shapes = compare(args.replay, group)
        prefixes += count
        kinds |= records
        primitives |= shapes
    print(json.dumps({'native_prefixes': prefixes, 'independent_scopes': len(groups),
                      'record_kinds': sorted(kinds), 'scene_primitives': sorted(primitives), 'passed': True}))


if __name__ == '__main__':
    main()
