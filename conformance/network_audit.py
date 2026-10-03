#!/usr/bin/env python3
"""Independent audit of explicitly authorized native post-auth packet traces.

Input is the bounded JSONL diagnostic artifact, never credentials or TLS keys.
Publication prefixes are replayed through the full handwritten and Rust reducers.
"""
import argparse
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path

from .full_interop import compare, verify_resource
from .reference import fields, one, many, number, string

MAX_FILE = 128 << 20
MAX_PACKET = 1 << 20
MAX_PUBLICATION = 4 << 20
MAX_RESOURCES = 512
HOST_KINDS = {2, 4, 6, 7, 8, 10, 11, 12, 13, 14, 20, 22, 23, 33, 34, 41, 44, 54}
CLIENT_KINDS = {3, 5, 6, 7, 21, 24, 25, 30, 31, 32, 35, 40, 42, 43, 50, 51, 52, 53, 55}


def identity(message, tag):
    value = one(message, tag, 2)
    if not isinstance(value, bytes) or len(value) != 16:
        raise ValueError('invalid trace identity')
    return value


def assemble(transfer, identifier, index, data):
    if identifier != transfer['id'] or not 0 <= index < transfer['count'] or not data:
        raise ValueError('invalid transfer chunk')
    chunks = transfer['chunks']
    if index < len(chunks):
        if chunks[index] != data:
            raise ValueError('conflicting duplicate chunk')
        return
    if index != len(chunks) or transfer['received'] + len(data) > transfer['total']:
        raise ValueError('out-of-order or oversized transfer')
    chunks.append(data)
    transfer['received'] += len(data)


def complete(transfer):
    if len(transfer['chunks']) != transfer['count'] or transfer['received'] != transfer['total']:
        raise ValueError('incomplete transfer')
    return b''.join(transfer['chunks'])


def audit(path, replay):
    if path.stat().st_size > MAX_FILE:
        raise ValueError('diagnostic file ceiling')
    publications = defaultdict(list)
    resources = defaultdict(set)
    commands, applied_results = {}, {}
    per_scope_sequences = defaultdict(dict)
    applied_revisions = defaultdict(int)
    counts = Counter()
    scope = None
    segment = None
    sequence = 0
    pending = None
    waiting = []
    offers = {}
    acknowledged = 0
    file_hash = hashlib.sha256()
    expected_binding = None
    segment_ended = False

    def drain():
        while waiting:
            item = waiting[0]
            if not set(item['available']) <= resources[scope]:
                return
            publications[scope].append(waiting.pop(0))
            if len(publications[scope]) > 4096:
                raise ValueError('publication history ceiling')
            counts['complete_publications'] += 1

    with path.open('rb') as stream:
        for line_number in range(32769):
            line = stream.readline(2 * MAX_PACKET + 8193)
            if not line:
                break
            if len(line) > 2 * MAX_PACKET + 8192 or line_number == 32768:
                raise ValueError('diagnostic record ceiling')
            file_hash.update(line)
            entry = json.loads(line)
            if entry['record'] == 'scope':
                if entry['segment'] != counts['segments'] + 1 or segment is not None and not segment_ended:
                    raise ValueError('connection segment ordering or missing boundary')
                counts['interrupted_publications'] += int(pending is not None) + len(waiting)
                counts['incomplete_resource_transfers'] += sum(t['received'] > 0 for t in offers.values())
                counts['unrequested_resource_offers'] += sum(t['received'] == 0 for t in offers.values())
                if counts['segments'] >= 32:
                    raise ValueError('connection segment ceiling')
                counts['segments'] += 1
                scope = tuple(bytes.fromhex(entry[key]) for key in ('session_id', 'epoch', 'principal_id', 'view_id'))
                if any(len(value) != 16 for value in scope):
                    raise ValueError('invalid scoped recording')
                expected_binding = scope[0], scope[1], scope[3]
                segment, sequence = entry['segment'], 0
                segment_ended = False
                pending, waiting, offers = None, [], {}
                acknowledged = applied_revisions[scope]
                continue
            if entry['record'] == 'segment-end':
                if scope is None or segment_ended or entry['segment'] != segment or entry['through_sequence'] != sequence:
                    raise ValueError('invalid retained prefix boundary')
                segment_ended = True
                counts['closed_prefixes'] += 1
                continue
            if entry['record'] != 'packet' or scope is None or segment_ended or entry['segment'] != segment:
                raise ValueError('unscoped diagnostic packet')
            if entry['sequence'] != sequence + 1:
                raise ValueError('diagnostic packet gap or duplicate')
            sequence += 1
            raw = bytes.fromhex(entry['exact_packet_hex'])
            if len(raw) != entry['wire_bytes'] or len(raw) > MAX_PACKET:
                raise ValueError('packet length mismatch')
            if 'sha256' in entry and entry['sha256'] != hashlib.sha256(raw).hexdigest():
                raise ValueError('diagnostic packet digest mismatch')
            packet = fields(raw)
            if tuple(identity(packet, tag) for tag in (1, 2, 3)) != expected_binding or number(packet, 4) != 1:
                raise ValueError('packet scope mismatch')
            kind = number(packet, 5)
            if kind != entry['kind'] or kind == 1 or many(packet, 7, 2):
                raise ValueError('credential, unsupported feature or wrong diagnostic kind')
            payload = one(packet, 6, 2, b'')
            body = fields(payload)
            direction = entry['direction']
            if direction not in ('host-to-client', 'client-to-host'):
                raise ValueError('invalid packet direction')
            host = direction == 'host-to-client'
            if kind not in (HOST_KINDS if host else CLIENT_KINDS):
                raise ValueError('wrong-direction protocol message')
            counts['packets'] += 1
            counts['exact_packet_bytes'] += len(raw)
            if host and kind in (10, 11):
                total, count = number(body, 4), number(body, 5)
                if pending is not None or not 0 < total <= MAX_PUBLICATION or not 0 < count <= 4096:
                    raise ValueError('publication begin bounds')
                digest = one(body, 7, 2)
                if digest is None or len(digest) != 32:
                    raise ValueError('publication digest')
                pending = {'id': identity(body, 1), 'kind': 'snapshot' if kind == 10 else 'transaction',
                           'base': number(body, 2), 'revision': number(body, 3), 'total': total,
                           'count': count, 'items': number(body, 6), 'digest': digest,
                           'chunks': [], 'received': 0}
            elif host and kind == 12:
                if pending is None:
                    raise ValueError('publication chunk without begin')
                assemble(pending, identity(body, 1), number(body, 2), one(body, 3, 2))
            elif host and kind == 13:
                if pending is None or (identity(body, 1), number(body, 2), number(body, 3), number(body, 4), one(body, 5, 2)) != (
                        pending['id'], pending['total'], pending['count'], pending['items'], pending['digest']):
                    raise ValueError('publication commit metadata mismatch')
                raw_publication = complete(pending)
                if hashlib.sha256(raw_publication).digest() != pending['digest']:
                    raise ValueError('publication payload digest mismatch')
                value = fields(raw_publication)
                snapshot = pending['kind'] == 'snapshot'
                if (number(value, 1 if snapshot else 2) != pending['revision']
                        or len(many(value, 2 if snapshot else 3, 2)) != pending['items']
                        or not snapshot and number(value, 1) != pending['base']):
                    raise ValueError('decoded publication metadata mismatch')
                waiting.append({'kind': pending['kind'], 'hex': raw_publication.hex(),
                                'available': [digest.hex() for digest in many(value, 3 if snapshot else 4, 2)],
                                'revision': pending['revision'], 'file': f'{path.name}:{line_number + 1}'})
                if len(waiting) > 8:
                    raise ValueError('resource-wait publication ceiling')
                pending = None
                drain()
            elif host and kind == 14:
                if pending is None or identity(body, 1) != pending['id']:
                    raise ValueError('unmatched publication abort')
                pending = None
                counts['aborted_publications'] += 1
            elif host and kind == 20:
                digest = one(body, 1, 2)
                if digest is None or len(digest) != 32 or digest not in offers and len(offers) >= MAX_RESOURCES:
                    raise ValueError('resource offer ceiling')
                total, chunks = number(body, 4), number(body, 5)
                if not 0 < total <= MAX_PUBLICATION or not 0 < chunks <= 4096:
                    raise ValueError('resource transfer ceiling')
                if (number(body, 2), number(body, 3), number(body, 6)) not in ((1, 1, 1), (2, 1, 2)):
                    raise ValueError('unsupported resource descriptor')
                if digest in offers and offers[digest]['received']:
                    raise ValueError('resource offer replaced an incomplete transfer')
                offers[digest] = {'id': digest, 'kind': number(body, 2), 'version': number(body, 3),
                                  'provenance': number(body, 6), 'total': total, 'count': chunks,
                                  'received': 0, 'chunks': []}
            elif host and kind == 22:
                digest = one(body, 1, 2)
                if digest not in offers:
                    raise ValueError('resource chunk without offer')
                assemble(offers[digest], digest, number(body, 2), one(body, 3, 2))
            elif host and kind == 23:
                digest = one(body, 1, 2)
                transfer = offers.pop(digest)
                vector = complete(transfer)
                verify_resource(vector, transfer['kind'], transfer['version'], transfer['provenance'], digest.hex())
                resources[scope].add(digest.hex())
                if len(resources[scope]) > MAX_RESOURCES:
                    raise ValueError('verified resource ceiling')
                counts['verified_resource_transfers'] += 1
                counts['verified_vector_bytes'] += len(vector)
                drain()
            elif not host and kind == 25:
                resources[scope].difference_update(digest.hex() for digest in many(body, 1, 2))
            elif not host and kind in (30, 31, 35):
                revision = number(body, 1)
                known = {item['revision']: item for item in publications[scope]}
                if revision not in known:
                    raise ValueError('readiness/presentation without audited complete publication')
                if not set(known[revision]['available']) <= resources[scope]:
                    raise ValueError('readiness/presentation after unresolved resource loss')
                if kind == 31 and revision > acknowledged:
                    raise ValueError('Presented before Applied')
                if kind == 30:
                    acknowledged = max(acknowledged, revision)
                    applied_revisions[scope] = acknowledged
                counts['presented_acknowledgements' if kind == 31 else 'readiness_acknowledgements'] += 1
            elif not host and kind in (50, 51, 52, 53):
                header = body if kind == 50 else fields(one(body, 1, 2))
                if (identity(header, 1), identity(header, 2)) != (scope[1], scope[3]):
                    raise ValueError('command scope mismatch')
                producer, command_sequence = identity(header, 3), number(header, 4)
                key = scope[:3], producer, command_sequence
                exact = kind, hashlib.sha256(payload).digest()
                if key in commands:
                    if commands[key] != exact:
                        raise ValueError('command identity reused for different exact bytes')
                    counts['exact_command_retries'] += 1
                else:
                    commands[key] = exact
                    counts['commands'] += 1
                per_scope_sequences[scope][command_sequence] = key
                counts[f'command_family_{kind}'] += 1
            elif host and kind == 54:
                command_sequence = number(body, 1)
                key = per_scope_sequences[scope].get(command_sequence)
                if key is None:
                    raise ValueError('result without a recorded command')
                status = string(body, 2)
                if status not in ('Accepted', 'Dispatched', 'Applied', 'Rejected', 'Failed', 'OutcomeUnknown', 'OutcomeExpired'):
                    raise ValueError('unknown command outcome')
                if status == 'Applied':
                    evidence = string(body, 4)
                    if not evidence:
                        raise ValueError('Applied without source evidence')
                    outcome = evidence, number(body, 5) if 5 in body else None
                    if key in applied_results and applied_results[key] != outcome:
                        raise ValueError('exact retry changed applied source evidence')
                    applied_results[key] = outcome
                counts['command_results'] += 1
            elif kind not in (2, 3, 4, 5, 6, 7, 8, 21, 24, 32, 33, 34, 40, 41, 42, 43, 44, 55):
                raise ValueError('unhandled packet kind or direction')
        else:
            raise ValueError('diagnostic record ceiling')
    counts['interrupted_publications'] += int(pending is not None) + len(waiting)
    counts['incomplete_resource_transfers'] += sum(t['received'] > 0 for t in offers.values())
    counts['unrequested_resource_offers'] += sum(t['received'] == 0 for t in offers.values())
    kinds, shapes = set(), set()
    for group in publications.values():
        _, record_kinds, primitives = compare(replay, group)
        kinds |= record_kinds
        shapes |= primitives
    if not counts['complete_publications']:
        raise ValueError('trace contains no auditable complete publication')
    return {'passed': True, 'sha256': file_hash.hexdigest(), **dict(counts),
            'record_kinds': sorted(kinds), 'scene_primitives': sorted(shapes)}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('trace', type=Path)
    parser.add_argument('--replay', type=Path, required=True)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--expected', type=Path, help='exact retained audit summary for deterministic replay')
    args = parser.parse_args()
    result = audit(args.trace, args.replay)
    if args.expected:
        if args.expected.stat().st_size > 65536 or json.loads(args.expected.read_text()) != result:
            raise ValueError('retained network audit summary differs')
    encoded = json.dumps(result, indent=2) + '\n'
    if args.output:
        args.output.write_text(encoded)
    print(encoded, end='')


if __name__ == '__main__':
    main()
