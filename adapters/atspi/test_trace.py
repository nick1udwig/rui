"""Independent handwritten decoder of the exact native accessibility trace."""
import hashlib
import json
from pathlib import Path
import unittest

from conformance.semantic_client import SemanticReplica, binary, decode


class NativeTraceTests(unittest.TestCase):
    def test_exact_native_gap_race_retirement_and_restart(self):
        root = Path(__file__).resolve().parents[2] / 'fixtures/atspi/golden'
        report = json.loads((root / 'report.json').read_text())
        replica = SemanticReplica()
        binding = None
        initial = None
        survivor = None
        for capture in report['captures']:
            name = capture['name']
            file = name + '.bin'
            raw = (root / file).read_bytes()
            self.assertEqual(hashlib.sha256(raw).hexdigest(), report['files'][file])
            envelope = decode(raw, set(range(1, 8)))
            scope = tuple(binary(envelope, key) for key in (2, 3, 4))
            if binding is None:
                binding = scope
            self.assertEqual(binding, scope)
            replica.apply(1, binary(envelope, 6))
            self.assertEqual(replica.revision, capture['revision'])
            ids = set(replica.records)
            labels = {r['label'] for r in replica.records.values() if r['kind'] == 3}
            if name == 'initial':
                initial = ids
            elif name == 'event-gap':
                self.assertIn('Replacement 1', labels)
                self.assertEqual(len(replica.retired), 2)
            elif name == 'crawl-race':
                self.assertIn('Replacement 2', labels)
                self.assertEqual(len(replica.retired), 4)
            elif name == 'source-exited':
                survivor = ids
                self.assertEqual(len(ids), 28)
                self.assertLess(ids, initial)
            elif name == 'source-restarted':
                self.assertEqual(initial & ids, survivor)
                self.assertEqual(len(ids), 55)
        self.assertEqual(replica.revision, 6)
        self.assertEqual(len(replica.retired), 31)


if __name__ == '__main__':
    unittest.main()
