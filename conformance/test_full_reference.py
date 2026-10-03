import copy
import hashlib
import struct
import unittest

from .full_reference import FullReplica, record
from .full_interop import verify_resource
from .semantic_client import blob, scalar
from .test_semantic_client import node, ref, snapshot


class FullReferenceTests(unittest.TestCase):
    def test_failure_preserves_records_and_retired_ids(self):
        replica = FullReplica()
        replica.apply('snapshot', snapshot([node(1, role=1, children=(2,)), node(2, label='Alive')]))
        before = copy.deepcopy(replica.state())
        invalid = scalar(1, 1) + scalar(2, 2) + blob(3, blob(2, ref(3, 2)))
        with self.assertRaises(ValueError):
            replica.apply('transaction', invalid)
        self.assertEqual(replica.state(), before)
        valid = scalar(1, 1) + scalar(2, 2) + blob(3, blob(1, node(1, version=2, role=1))) + blob(3, blob(2, ref(3, 2)))
        replica.apply('transaction', valid)
        self.assertEqual(replica.retired, {2})
        before = copy.deepcopy(replica.state())
        resurrect = scalar(1, 2) + scalar(2, 3) + blob(3, blob(1, node(2, version=9)))
        with self.assertRaises(ValueError):
            replica.apply('transaction', resurrect)
        self.assertEqual(replica.state(), before)

    def test_unknown_record_is_not_silently_discarded(self):
        with self.assertRaises(ValueError):
            record(blob(7, scalar(1, 99)))
        self.assertEqual(record(node(1) + blob(128, b'optional')), record(node(1)))

    def test_binding_all_references_and_optional_region_decode(self):
        raw = scalar(1, 20) + scalar(2, 3) + blob(3, ref(1, 2)) + blob(4, ref(3, 4)) + blob(5, ref(2, 5))
        value = record(blob(5, raw))['Binding']
        self.assertEqual(value, {'id': 20, 'version': 3, 'surface': {'kind': 'Surface', 'id': 2},
                                 'semantics': [{'kind': 'Semantic', 'id': 4}],
                                 'scenes': [{'kind': 'Scene', 'id': 5}], 'interaction_region': None})

    def test_exact_resource_header_geometry_and_trailing_bytes(self):
        def digest(body, kind=1):
            return hashlib.sha256(b'RUI/0.1/resource\0' + struct.pack('>IIQI', kind, 1, len(body), kind) + body).hexdigest()
        valid = b'RVP1' + struct.pack('>I', 2) + b'\x01' + struct.pack('>dd', 1, 2) + b'\x05'
        verify_resource(valid, 1, 1, 1, digest(valid))
        bad = [valid + b'opaque', b'PNG\x00' + valid[4:], b'RVP1' + struct.pack('>I', 1) + b'\x05',
               b'RVP1' + struct.pack('>I', 1) + b'\x01' + struct.pack('>dd', float('nan'), 0)]
        for body in bad:
            with self.subTest(body=body), self.assertRaises(ValueError):
                verify_resource(body, 1, 1, 1, digest(body))
        with self.assertRaises(ValueError):
            verify_resource(valid, 2, 1, 2, digest(valid))


if __name__ == '__main__':
    unittest.main()
