"""Barrier quota/cancellation regressions; native coherence uses whole_check."""
import unittest
from types import SimpleNamespace
from adapters.gtk4.transaction import CaptureBudget, CaptureTransaction, BarrierRejected, METADATA_CHARGE


class TransactionLifecycle(unittest.TestCase):
    def setUp(self):
        self.now = 0
        self.source = SimpleNamespace(identity='01' * 16, source_revision=1)
        self.captures = {'a': object(), 'b': object()}
        self.commits = []
        self.budget = CaptureBudget()
        self.barrier = CaptureTransaction(self.source, self.oracle, lambda *_: None,
            lambda nonce, facts, frames, offers: self.commits.append((nonce, set(frames))),
            budget=self.budget, clock=lambda: self.now, max_restarts=1)
        for key, capture in self.captures.items():
            self.barrier.register(key, capture)

    def tearDown(self):
        self.barrier.close()
        self.assertEqual(self.budget.used, 0)

    def oracle(self):
        return {'incarnation': self.source.identity, 'source_revision': self.source.source_revision,
            'surfaces': {key: {'alive': True, 'mapped': True, 'semantics': []} for key in self.captures}}

    def frame(self, ticket):
        return {'source_revision': self.source.source_revision, 'capture_boundary': 'actual-snapshot-vfunc-same-node-append',
            'capture_barrier_nonce': ticket.nonce, 'capture_token': 'native-ticket-' + str(ticket.nonce),
            'scene': [], 'semantics': []}

    def offer(self, size=8, kind=1):
        return {'digest': 'ab' * 32, 'total_bytes': size, 'kind': kind, 'provenance': kind, 'version': 1}

    def test_candidate_and_union_reservations_release_on_cancel_and_late_reply(self):
        baseline = self.budget.used
        for _ in range(20):
            self.barrier.begin()
            ticket = self.barrier.acquire('a')
            self.barrier.submit(ticket, self.captures['a'], self.frame(ticket), [self.offer()])
            self.assertGreater(self.budget.used, baseline)
            self.barrier.cancel('source-disconnect')
            self.assertEqual(self.budget.used, baseline)
            self.assertFalse(self.barrier.submit(ticket, self.captures['a'], self.frame(ticket), []))
        self.assertFalse(self.commits)

    def test_timeout_without_any_native_frame_releases_all_transient_memory(self):
        baseline = self.budget.used
        self.barrier.begin()
        self.now = 3
        self.assertTrue(self.barrier.expire())
        self.assertEqual(self.budget.used, baseline)
        self.assertFalse(self.barrier.expire())

    def test_export_scratch_quota_failure_cancels_immediately_after_native_append(self):
        # Actual native GSK node and Gtk.Snapshot; no exporter allocation or
        # frame sampling is needed to exercise this failure boundary.
        from adapters.gtk4.capture import CaptureMixin, Gsk, Gtk
        from gi.repository import Gdk, Graphene
        capture = CaptureMixin()
        capture.capture_transaction = self.barrier
        capture.surface_key = 'a'
        self.captures['a'] = capture
        _, slot = self.barrier.registered['a']
        self.barrier.registered['a'] = (capture, slot)
        baseline = self.budget.used
        self.budget.limit = baseline + 3 * METADATA_CHARGE
        self.barrier.begin()
        color = Gdk.RGBA(); color.parse('#ffffff')
        node = Gsk.ColorNode.new(color, Graphene.Rect().init(0, 0, 8, 8))
        snapshot = Gtk.Snapshot()
        with self.assertRaisesRegex(BarrierRejected, 'capture-memory-ceiling'):
            capture.capture_node(snapshot, node, self.source.source_revision, 0)
        self.assertIsNotNone(snapshot.to_node())
        self.assertFalse(self.barrier.active)
        self.assertEqual(self.budget.used, baseline)
        self.assertFalse(self.commits)

    def test_exhausted_candidate_quota_rolls_back_all_partial_reservations(self):
        baseline = self.budget.used
        self.budget.limit = baseline + 2 * METADATA_CHARGE
        with self.assertRaisesRegex(BarrierRejected, 'memory-ceiling'):
            self.barrier.begin()
        self.assertEqual(self.budget.used, baseline)
        self.assertFalse(self.barrier.active)

    def test_conflicting_union_offer_aborts_without_commit_or_retained_resources(self):
        baseline = self.budget.used
        self.barrier.begin()
        first = self.barrier.acquire('a')
        self.barrier.submit(first, self.captures['a'], self.frame(first), [self.offer()])
        second = self.barrier.acquire('b')
        with self.assertRaisesRegex(BarrierRejected, 'conflicting-source-descriptor'):
            self.barrier.submit(second, self.captures['b'], self.frame(second), [self.offer(kind=2)])
        self.assertEqual(self.budget.used, baseline)
        self.assertFalse(self.commits)

    def test_restart_ceiling_and_stale_candidate_cannot_complete_new_transaction(self):
        self.barrier.begin()
        old = self.barrier.acquire('a')
        self.source.source_revision += 1
        self.barrier.acquire('b')
        self.assertFalse(self.barrier.submit(old, self.captures['a'], self.frame(old), []))
        self.source.source_revision += 1
        with self.assertRaisesRegex(BarrierRejected, 'restart-or-deadline-ceiling'):
            self.barrier.acquire('b')
        self.assertFalse(self.barrier.active)
        self.assertFalse(self.commits)

    def test_synchronous_candidate_restart_does_not_release_new_tickets(self):
        baseline = self.budget.used
        old = []
        def restart(nonce, key):
            old.append(self.barrier.tickets[key])
            self.barrier.on_candidate = None
            self.source.source_revision += 1
            self.barrier.invalidate('synchronous-native-lifecycle')
        self.barrier.on_candidate = restart
        self.barrier.begin()
        ticket = self.barrier.acquire('a')
        self.barrier.submit(ticket, self.captures['a'], self.frame(ticket), [self.offer()])
        self.assertTrue(self.barrier.active)
        self.assertEqual(self.barrier.nonce, 2)
        new = self.barrier.acquire('a')
        self.assertIsNot(new, ticket)
        retained = self.budget.used
        self.assertFalse(self.barrier.submit(ticket, self.captures['a'], self.frame(ticket), []))
        self.assertEqual(self.budget.used, retained)
        for key in ('a', 'b'):
            fresh = self.barrier.acquire(key)
            self.barrier.submit(fresh, self.captures[key], self.frame(fresh), [])
        self.assertEqual(self.commits, [(2, {'a', 'b'})])
        self.assertEqual(self.budget.used, baseline)

    def test_synchronous_snapshot_request_restart_preserves_new_transaction(self):
        baseline = self.budget.used
        requests = []
        def request(nonce, required):
            requests.append((nonce, set(required)))
            if nonce == 1:
                self.source.source_revision += 1
                self.barrier.invalidate('mapping-changed-during-native-request')
            else:
                for key in sorted(required):
                    ticket = self.barrier.acquire(key)
                    self.barrier.submit(ticket, self.captures[key], self.frame(ticket), [])
        self.barrier.request_snapshots = request
        self.barrier.begin()
        self.assertEqual(self.commits, [(2, {'a', 'b'})])
        self.assertEqual(requests, [(1, {'a', 'b'}), (2, {'a', 'b'})])
        self.assertFalse(self.barrier.active)
        self.assertEqual(self.budget.used, baseline)

    def test_failed_commit_releases_candidate_and_union_reservations(self):
        baseline = self.budget.used
        self.barrier.committed = lambda *_: (_ for _ in ()).throw(ValueError('consumer-refused'))
        self.barrier.begin()
        first = self.barrier.acquire('a')
        self.barrier.submit(first, self.captures['a'], self.frame(first), [self.offer()])
        second = self.barrier.acquire('b')
        with self.assertRaisesRegex(ValueError, 'consumer-refused'):
            self.barrier.submit(second, self.captures['b'], self.frame(second), [])
        self.assertEqual(self.budget.used, baseline)
        self.assertFalse(self.barrier.active)


if __name__ == '__main__':
    unittest.main()
