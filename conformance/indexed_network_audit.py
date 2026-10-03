#!/usr/bin/env python3
"""Feed indexed exact packet files into the existing independent network audit.

Scopes can be interleaved in an index. Their recorded per-connection order is
preserved; grouping makes no claim about ordering between connections. A segment
end marks the retained prefix boundary, not an observed WebSocket close.
An index must contain at most one connection per scope; same-scope reconnects
need separate indexes because this index format has no recorder identity.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import tempfile

from .network_audit import MAX_FILE, MAX_PACKET, audit

IDENTITIES = ('session_id', 'epoch', 'principal_id', 'view_id')
DIRECTIONS = {'ClientToHost': 'client-to-host', 'HostToClient': 'host-to-client'}


def convert(index_path, output):
    if index_path.is_symlink() or index_path.stat().st_size > 8 << 20:
        raise ValueError('index file ceiling or symbolic link')
    with index_path.open('rb') as stream:
        raw_index = stream.read((8 << 20) + 1)
    if len(raw_index) > 8 << 20:
        raise ValueError('index file ceiling')
    entries = json.loads(raw_index)
    if not isinstance(entries, list) or not 0 < len(entries) <= 32704:
        raise ValueError('index record ceiling')
    groups, files = {}, set()
    for entry in entries:
        if not isinstance(entry, dict):
            raise ValueError('invalid index entry')
        for key, limit in (('sequence', 32704), ('kind', 55), ('wire_bytes', MAX_PACKET)):
            if type(entry[key]) is not int or not 1 <= entry[key] <= limit:
                raise ValueError('invalid indexed integer')
        scope = tuple(entry[key] for key in IDENTITIES)
        if any(not isinstance(value, str) or not re.fullmatch('[0-9a-f]{32}', value)
               for value in scope):
            raise ValueError('invalid indexed scope')
        if scope not in groups:
            if len(groups) == 32:
                raise ValueError('index connection ceiling')
            groups[scope] = []
        group = groups[scope]
        if entry['sequence'] != len(group) + 1:
            raise ValueError('indexed packet gap or duplicate')
        filename = entry['file']
        if not isinstance(filename, str) or not re.fullmatch(r'packet-[0-9]{4,}\.bin', filename):
            raise ValueError('invalid indexed packet filename')
        if filename in files:
            raise ValueError('duplicate indexed file')
        files.add(filename)
        if entry['direction'] not in DIRECTIONS:
            raise ValueError('invalid indexed direction')
        group.append(entry)
    size = 0

    def emit(entry):
        nonlocal size
        line = json.dumps(entry, sort_keys=True, separators=(',', ':')) + '\n'
        size += len(line.encode())
        if size > MAX_FILE:
            raise ValueError('converted trace ceiling')
        output.write(line)

    for segment, (scope, group) in enumerate(groups.items(), 1):
        emit(dict(record='scope', segment=segment, **dict(zip(IDENTITIES, scope))))
        for entry in group:
            path = index_path.parent / entry['file']
            if path.is_symlink() or not path.is_file() or not 0 < path.stat().st_size <= MAX_PACKET:
                raise ValueError('packet file ceiling or symbolic link')
            with path.open('rb') as stream:
                raw = stream.read(MAX_PACKET + 1)
            if len(raw) > MAX_PACKET or len(raw) != entry['wire_bytes']:
                raise ValueError('indexed packet length mismatch')
            if hashlib.sha256(raw).hexdigest() != entry['sha256']:
                raise ValueError('indexed packet digest mismatch')
            emit(dict(record='packet', segment=segment, sequence=entry['sequence'],
                      direction=DIRECTIONS[entry['direction']], kind=entry['kind'],
                      wire_bytes=len(raw), sha256=entry['sha256'], exact_packet_hex=raw.hex()))
        emit(dict(record='segment-end', segment=segment, through_sequence=len(group)))
    return hashlib.sha256(raw_index).hexdigest()


def audit_index(index_path, replay):
    with tempfile.TemporaryDirectory(prefix='rui-indexed-audit-') as directory:
        converted = Path(directory) / 'trace.jsonl'
        with converted.open('w') as output:
            index_hash = convert(index_path, output)
        return dict(audit(converted, replay), index_sha256=index_hash)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('index', type=Path)
    parser.add_argument('--replay', type=Path, required=True)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--expected', type=Path)
    args = parser.parse_args()
    result = audit_index(args.index, args.replay)
    if args.expected and (args.expected.stat().st_size > 65536
                          or json.loads(args.expected.read_text()) != result):
        raise ValueError('retained indexed audit summary differs')
    encoded = json.dumps(result, indent=2, sort_keys=True) + '\n'
    if args.output:
        args.output.write_text(encoded)
    print(encoded, end='')


if __name__ == '__main__':
    main()
