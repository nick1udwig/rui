import hashlib
import json
import struct
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from .network_audit import audit
from .semantic_client import blob, scalar
from .test_semantic_client import node, snapshot


def fixture():
    ids = [bytes([i]) * 16 for i in range(1, 7)]
    session, epoch, principal, view, producer, lease = ids
    scope = dict(record='scope', segment=1, session_id=session.hex(), epoch=epoch.hex(),
                 principal_id=principal.hex(), view_id=view.hex())
    rows = [scope]

    def packet(kind, payload, host=True):
        raw = blob(1, session) + blob(2, epoch) + blob(3, view) + scalar(4, 1) + scalar(5, kind) + blob(6, payload)
        rows.append(dict(record='packet', segment=1, sequence=len(rows),
                         direction='host-to-client' if host else 'client-to-host', kind=kind,
                         wire_bytes=len(raw), exact_packet_hex=raw.hex()))
    publication = snapshot([node(1, role=1, label='Counter')])
    digest = hashlib.sha256(publication).digest()
    publication_id = b'P' * 16
    packet(10, blob(1, publication_id) + scalar(3, 1) + scalar(4, len(publication)) + scalar(5, 1) + scalar(6, 1) + blob(7, digest))
    packet(12, blob(1, publication_id) + blob(3, publication))
    packet(13, blob(1, publication_id) + scalar(2, len(publication)) + scalar(3, 1) + scalar(4, 1) + blob(5, digest))
    packet(30, scalar(1, 1), False)
    packet(31, scalar(1, 1), False)
    command = blob(1, epoch) + blob(2, view) + blob(3, producer) + scalar(4, 1) + blob(5, lease) + scalar(6, 1)
    command += scalar(7, 1) + blob(8, 'Activate') + scalar(9, 1) + scalar(10, 1) + blob(12, b'')
    result = scalar(1, 1) + blob(2, 'Applied') + blob(4, 'source-proof') + scalar(5, 1)
    packet(50, command, False)
    packet(54, result)
    packet(50, command, False)
    packet(54, result)
    return rows


class NetworkAuditTests(unittest.TestCase):
    def run_audit(self, rows):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / 'trace.jsonl'
            path.write_text(''.join(json.dumps(row) + '\n' for row in rows))
            # These tests isolate packet/transfer/evidence auditing. The actual
            # full-record comparison is separately exercised on native artifacts.
            with patch('conformance.network_audit.compare', return_value=(1, {'Semantic'}, set())) as replay:
                result = audit(path, Path('unused-fixture-replay'))
                self.assertEqual(replay.call_count, 1)
                self.assertEqual(len(replay.call_args.args[1]), 1)
            return result

    def test_complete_prefix_readiness_and_exact_retry(self):
        result = self.run_audit(fixture())
        self.assertEqual(result['complete_publications'], 1)
        self.assertEqual(result['presented_acknowledgements'], 1)
        self.assertEqual(result['commands'], 1)
        self.assertEqual(result['exact_command_retries'], 1)
        self.assertEqual(result['command_results'], 2)

    def test_scope_sequence_and_wire_length_corruption_rejected(self):
        for key, value in [('segment', 9), ('sequence', 9), ('wire_bytes', 9), ('kind', 1)]:
            rows = fixture()
            rows[2][key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.run_audit(rows)

    def test_retained_prefix_boundaries_and_packet_hashes(self):
        rows = fixture()
        end = dict(record='segment-end', segment=1, through_sequence=len(rows)-1)
        rows.append(end)
        self.assertEqual(self.run_audit(rows)['closed_prefixes'], 1)
        for changed in (dict(end, through_sequence=0), dict(end, segment=2)):
            with self.assertRaises(ValueError):
                self.run_audit(rows[:-1] + [changed])
        with self.assertRaises(ValueError):
            self.run_audit(rows + [dict(rows[1], sequence=end['through_sequence']+1)])
        with self.assertRaises(ValueError):
            self.run_audit(rows[:-1] + [dict(rows[0], segment=2)])
        with self.assertRaises(ValueError):
            self.run_audit(rows + [dict(rows[0], segment=3)])
        rows = fixture()
        rows[1]['sha256'] = '00' * 32
        with self.assertRaises(ValueError):
            self.run_audit(rows)

    def test_packet_binding_and_exact_chunk_digest_rejected(self):
        for changed in (bytes([9]) * 16, b'Xounter'):
            rows = fixture()
            data = bytes.fromhex(rows[2]['exact_packet_hex'])
            original = bytes([1]) * 16 if len(changed) == 16 else b'Counter'
            data = data.replace(original, changed, 1)
            rows[2]['exact_packet_hex'] = data.hex()
            with self.assertRaises(ValueError):
                self.run_audit(rows)

    def test_presentation_before_applied_and_result_evidence_change_rejected(self):
        rows = fixture()
        rows[4], rows[5] = rows[5], rows[4]
        rows[4]['sequence'], rows[5]['sequence'] = 4, 5
        with self.assertRaises(ValueError):
            self.run_audit(rows)
        rows = fixture()
        data = bytes.fromhex(rows[-1]['exact_packet_hex']).replace(b'source-proof', b'changedproof')
        rows[-1]['exact_packet_hex'] = data.hex()
        with self.assertRaises(ValueError):
            self.run_audit(rows)

    def test_resource_transfer_verifies_descriptor_and_exact_vector_body(self):
        body = b'RVP1' + struct.pack('>I', 0)
        digest = hashlib.sha256(b'RUI/0.1/resource\0' + struct.pack('>IIQI', 1, 1, len(body), 1) + body).digest()
        for corrupted in (False, True):
            rows = fixture()
            prefix = []
            messages = [(20, blob(1, digest) + scalar(2, 1) + scalar(3, 1) + scalar(4, len(body)) + scalar(5, 1) + scalar(6, 1)),
                        (22, blob(1, digest) + blob(3, (b'PNG!' + body[4:]) if corrupted else body)),
                        (23, blob(1, digest))]
            for kind, payload in messages:
                raw = blob(1, b'\x01' * 16) + blob(2, b'\x02' * 16) + blob(3, b'\x04' * 16)
                raw += scalar(4, 1) + scalar(5, kind) + blob(6, payload)
                prefix.append(dict(record='packet', segment=1, sequence=0, direction='host-to-client',
                                   kind=kind, wire_bytes=len(raw), exact_packet_hex=raw.hex()))
            rows[1:1] = prefix
            for sequence, row in enumerate(rows[1:], 1):
                row['sequence'] = sequence
            if corrupted:
                with self.assertRaises(ValueError):
                    self.run_audit(rows)
            else:
                self.assertEqual(self.run_audit(rows)['verified_resource_transfers'], 1)


if __name__ == '__main__':
    unittest.main()
