"""No GTK dependency: exact retained native trace plus corrupted counterfactuals."""
import copy
import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest

from conformance.reference import fields, number, one, string
from conformance.semantic_client import blob, scalar
from fixtures.gtk.composition_trace import verify_controller


def encode(message):
    return b''.join(scalar(tag, value) if wire == 0 else blob(tag, value)
                    for tag, entries in message.items() for wire, value in entries)


def rewrite(row, change):
    packet = fields(bytes.fromhex(row['exact_packet_hex']))
    body = fields(one(packet, 6, 2))
    change(body)
    packet[6] = [(2, encode(body))]
    raw = encode(packet)
    row.update(exact_packet_hex=raw.hex(), wire_bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest())


class NativeCompositionTraceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # Override only while freezing a freshly produced, not yet retained
        # canonical corpus. CI always uses the checked-in exact trace.
        cls.path = Path(os.environ.get('RUI_COMPOSITION_TRACE', str(Path(__file__).parent / 'desktop-composition-golden' / 'controller.trace.jsonl')))
        cls.rows = [json.loads(line) for line in cls.path.read_text().splitlines()]

    def verify_rows(self, rows):
        with tempfile.TemporaryDirectory(prefix='rui-composition-counterfactual-') as temporary:
            path = Path(temporary) / 'controller.trace.jsonl'
            path.write_text(''.join(json.dumps(row) + '\n' for row in rows))
            return verify_controller(path)

    def test_native_wire_family_scalar_clocks_and_own_surface_cause(self):
        result = verify_controller(self.path)
        self.assertEqual((result['begins'], result['updates'], len(result['commits']), result['cancels']), (5, 12, 4, 1))
        self.assertEqual([item['text_version'] for item in result['commits']], [3, 5, 7, 9])
        self.assertEqual([item['text_record_version'] for item in result['commits']], [4, 7, 10, 13])
        self.assertEqual([item['caret'] for item in result['commits']], [1, 2, 3, 4])
        self.assertEqual(result['physical_composing_batches'], 0)
        self.assertEqual(result['semantic_replace_text_fallbacks'], 0)

    def test_incremental_public_clocks_and_native_formula_are_distinct(self):
        path = Path(__file__).parent / 'shared-actor' / 'controller.trace.jsonl'
        result = verify_controller(path, clock_policy='standard-public-clocks')
        self.assertEqual([item['text_version'] for item in result['commits']], [2, 3, 4, 5])
        self.assertEqual([item['text_record_version'] for item in result['commits']], [2, 3, 4, 5])
        self.assertEqual([item['content'] for item in result['commits']], ['a', 'aé', 'aéx', 'aéxy'])
        with self.assertRaises(ValueError):
            verify_controller(path)
        with self.assertRaises(ValueError):
            verify_controller(self.path, clock_policy='standard-public-clocks')
        with self.assertRaises(ValueError):
            verify_controller(path, clock_policy='unknown')

    def test_foreign_cause_and_transient_applied_are_rejected(self):
        for transient in (False, True):
            rows = copy.deepcopy(self.rows)
            for row in rows:
                if row.get('kind') != 54:
                    continue
                body = fields(one(fields(bytes.fromhex(row['exact_packet_hex'])), 6, 2))
                if string(body, 2) == ('Dispatched' if transient else 'Applied') and (transient or number(body, 1) > 1):
                    rewrite(row, lambda body: body.update({2: [(2, b'Applied')], 4: [(2, b'foreign-source-cause')]}))
                    break
            with self.subTest(transient=transient), self.assertRaises(ValueError):
                self.verify_rows(rows)

    def test_rebased_content_clock_and_scalar_range_are_required(self):
        for wrong_range in (False, True):
            rows = copy.deepcopy(self.rows)
            for row in rows:
                if row.get('kind') != 52:
                    continue
                body = fields(one(fields(bytes.fromhex(row['exact_packet_hex'])), 6, 2))
                if 6 in body and number(body, 5) == 7:
                    def corrupt(body):
                        if wrong_range:
                            body[6] = [(2, scalar(1, 2) + scalar(2, 2))]
                        else:
                            body[5] = [(0, 5)]
                    rewrite(row, corrupt)
                    break
            with self.subTest(wrong_range=wrong_range), self.assertRaises(ValueError):
                self.verify_rows(rows)

    def test_unpainted_next_revision_and_semantic_fallback_are_rejected(self):
        for fallback in (False, True):
            rows = copy.deepcopy(self.rows)
            for row in rows:
                if row.get('kind') != (50 if fallback else 31):
                    continue
                body = fields(one(fields(bytes.fromhex(row['exact_packet_hex'])), 6, 2))
                if fallback or number(body, 1) == 5:
                    rewrite(row, lambda body: body.update({8: [(2, b'ReplaceText')]} if fallback else {1: [(0, 4)]}))
                    break
            with self.subTest(fallback=fallback), self.assertRaises(ValueError):
                self.verify_rows(rows)


if __name__ == '__main__':
    unittest.main()
