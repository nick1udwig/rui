"""Atomicity and lifetime exhaustion at the observed public boundary."""
import copy
import unittest
from unittest.mock import Mock, patch

from adapters.atspi.observer import ObservedProjection, WorkerPort


def candidate():
    root = [':1.2', '/app']
    frame = [':1.2', '/window']
    child = [':1.2', '/button']
    records = [{'key': frame, 'parent': root, 'children': [child], 'name': 'Window', 'role': 23},
               {'key': child, 'parent': frame, 'children': [], 'name': 'Hello', 'role': 43}]
    return {'type': 'snapshot', 'classification': 'OBSERVED', 'apps': [
        {'pid': 42, 'root': root, 'frames': [{'root': frame, 'bounds': [0, 0, 50, 50], 'records': records}]}]}


class ProjectionTests(unittest.TestCase):
    def setUp(self):
        self.projection = ObservedProjection()
        self.scope = {42: 'a' * 32}
        self.projection.publish(candidate(), self.scope)

    def assert_rejected_atomically(self, value, scope=None):
        before = copy.deepcopy(vars(self.projection))
        with self.assertRaises((ValueError, KeyError)):
            self.projection.publish(value, self.scope if scope is None else scope)
        self.assertEqual(vars(self.projection), before)

    def test_race_cycle_duplicate_parent_scope_and_private_label_rejected(self):
        for mode in ('cycle', 'duplicate', 'parent', 'scope', 'private', 'label', 'bounds'):
            with self.subTest(mode=mode):
                value = candidate()
                frame = value['apps'][0]['frames'][0]
                if mode == 'cycle':
                    frame['records'][1]['children'] = [frame['root']]
                elif mode == 'duplicate':
                    frame['records'].append(copy.deepcopy(frame['records'][1]))
                elif mode == 'parent':
                    frame['records'][1]['parent'] = [':1.2', '/other']
                elif mode == 'scope':
                    value['apps'][0]['root'][0] = ':9.9'
                elif mode == 'private':
                    frame['records'][1]['role'] = 40
                elif mode == 'label':
                    frame['records'][1]['name'] = 'X' * 257
                elif mode == 'bounds':
                    frame['bounds'][0] = float('nan')
                self.assert_rejected_atomically(value)

    def test_retired_object_path_and_source_restart_never_reuse_public_ids(self):
        old = set(self.projection.ids.values())
        self.projection.publish({'type': 'snapshot', 'classification': 'OBSERVED', 'apps': []}, {})
        self.assertEqual(self.projection.ids, {})
        self.projection.publish(candidate(), self.scope)
        self.assertFalse(old & set(self.projection.ids.values()))
        old |= set(self.projection.ids.values())
        self.projection.publish(candidate(), {42: 'b' * 32})
        self.assertFalse(old & set(self.projection.ids.values()))

    def test_identity_and_incarnation_lifetime_exhaustion_is_atomic(self):
        self.projection.next_id = self.projection.MAX_ISSUED + 1
        self.assert_rejected_atomically(candidate(), {42: 'b' * 32})
        self.projection.incarnations = {f'{i:032x}' for i in range(8)}
        self.assert_rejected_atomically(candidate(), {42: 'c' * 32})

    def test_no_coherent_claim_or_untrusted_source_scope(self):
        value = candidate()
        value['classification'] = 'COHERENT'
        self.assert_rejected_atomically(value)
        self.assert_rejected_atomically(candidate(), {43: 'a' * 32})


class PortTests(unittest.TestCase):
    def port(self, reply):
        port = WorkerPort.__new__(WorkerPort)
        port.process = Mock()
        port.process.poll.return_value = None
        port.channel = Mock()
        port.channel.recv.return_value = reply
        port.pending, port.barrier, port.deadline = 1, False, 100
        return port

    def test_deadline_kills_without_waiting_in_publication_poll(self):
        port = self.port(b'')
        with patch('adapters.atspi.observer.time.monotonic', return_value=101):
            self.assertEqual(port.poll()['reason'], 'worker-deadline')
        port.process.kill.assert_called_once()
        port.process.wait.assert_not_called()
        self.assertIsNone(port.pending)

    def test_stale_and_malformed_reply_abort_owner_without_publication(self):
        for reply in (b'{', b'[]', b'{"request_id":2}', b'\xff'):
            port = self.port(reply)
            with patch('adapters.atspi.observer.time.monotonic', return_value=99), self.assertRaises(ValueError):
                port.poll()
            port.process.kill.assert_called_once()
            self.assertIsNone(port.pending)


if __name__ == '__main__':
    unittest.main()
