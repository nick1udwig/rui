import copy
import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest

from .indexed_network_audit import convert, IDENTITIES
from . import test_network_audit as network_tests


class IndexedNetworkAuditTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.index = self.root / 'index.json'
        rows = network_tests.fixture()
        self.entries = []
        for row in rows[1:]:
            raw = bytes.fromhex(row['exact_packet_hex'])
            filename = f'packet-{row["sequence"]:04}.bin'
            (self.root / filename).write_bytes(raw)
            self.entries.append(dict(
                **{key: rows[0][key] for key in IDENTITIES}, file=filename,
                sequence=row['sequence'], kind=row['kind'], wire_bytes=len(raw),
                sha256=hashlib.sha256(raw).hexdigest(),
                direction='HostToClient' if row['direction'] == 'host-to-client' else 'ClientToHost'))

    def converted(self, entries):
        self.index.write_text(json.dumps(entries))
        output = io.StringIO()
        convert(self.index, output)
        return [json.loads(line) for line in output.getvalue().splitlines()]

    def test_exact_packets_use_existing_auditor(self):
        rows = self.converted(self.entries)
        for actual, expected in zip(rows[1:-1], network_tests.fixture()[1:]):
            self.assertEqual(actual['exact_packet_hex'], expected['exact_packet_hex'])
            self.assertEqual(actual['sequence'], expected['sequence'])
            self.assertEqual(actual['direction'], expected['direction'])
        result = network_tests.NetworkAuditTests().run_audit(rows)
        self.assertEqual(result['complete_publications'], 1)
        self.assertEqual(result['exact_command_retries'], 1)
        self.assertEqual(result['closed_prefixes'], 1)

    def test_interleaved_scopes_preserve_each_connection_order(self):
        interleaved = []
        for entry in self.entries:
            second = dict(entry, principal_id='ab' * 16, file='packet-1' + entry['file'][7:])
            (self.root / second['file']).write_bytes((self.root / entry['file']).read_bytes())
            interleaved.extend([entry, second])
        rows = self.converted(interleaved)
        scopes = [row for row in rows if row['record'] == 'scope']
        self.assertEqual([row['segment'] for row in scopes], [1, 2])
        for segment in (1, 2):
            packets = [row for row in rows if row['record'] == 'packet' and row['segment'] == segment]
            self.assertEqual([row['sequence'] for row in packets], list(range(1, 10)))

    def test_bad_index_and_path_redirection_rejected(self):
        for key, value in [('sequence', 2), ('sequence', True), ('kind', True),
                           ('wire_bytes', True), ('view_id', 'bad'), ('direction', 'Unknown'),
                           ('file', '../outside.bin'), ('file', '/tmp/packet-0001.bin'),
                           ('sha256', '00' * 32), ('wire_bytes', 1)]:
            entries = copy.deepcopy(self.entries)
            entries[0][key] = value
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                self.converted(entries)
        entries = copy.deepcopy(self.entries)
        entries[1]['file'] = entries[0]['file']
        with self.assertRaises(ValueError):
            self.converted(entries)
        first = self.root / self.entries[0]['file']
        first.unlink()
        first.symlink_to(self.root / self.entries[1]['file'])
        with self.assertRaises(ValueError):
            self.converted(self.entries)

    def test_same_scope_reconnect_requires_a_separate_index(self):
        repeated = []
        for entry in self.entries:
            other = dict(entry, file='packet-2' + entry['file'][7:])
            (self.root / other['file']).write_bytes((self.root / entry['file']).read_bytes())
            repeated.append(other)
        with self.assertRaises(ValueError):
            self.converted(self.entries + repeated)

    def test_changed_exact_packet_rejected_before_protocol_audit(self):
        path = self.root / self.entries[0]['file']
        raw = path.read_bytes()
        path.write_bytes(raw[:-1] + bytes([raw[-1] ^ 1]))
        with self.assertRaises(ValueError):
            self.converted(self.entries)


if __name__ == '__main__':
    unittest.main()
