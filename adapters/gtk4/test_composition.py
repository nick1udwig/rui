"""Private transient-state cancellation and scalar editing boundaries."""
import time
import unittest
from types import SimpleNamespace
from unittest.mock import patch

from adapters.gtk4.composition import NativeComposition, MAX_TEXT_BYTES
from adapters.gtk4.state import SourceRejected
from adapters.gtk4.transaction import CaptureBudget
from adapters.gtk4.native import NativeDispatcher


class NativeCompositionTests(unittest.TestCase):
    def setUp(self):
        self.generation = 7
        self.context = {'content': 'Aé😀e\u0301Z', 'text_record_version': 3, 'focus_generation': 2}
        self.budget = CaptureBudget(limit=1 << 20)
        self.composition = NativeComposition(lambda _: dict(self.context), SimpleNamespace(current=lambda: self.generation), self.budget)
        self.guard = {'control_generation': 7, 'lease_id': 'lease', 'source_generation': 1,
                      'deadline_ns': time.monotonic_ns() + 5_000_000_000}

    def tearDown(self):
        self.composition.close()
        self.assertEqual(self.budget.used, 0)

    def request(self, stage, **fields):
        return dict(guard=dict(self.guard), text_session_id='01' * 16, stage=stage, **fields)

    def begin(self):
        self.composition.stage(self.request('begin', range=[1, 4]), dict(self.context))

    def test_preedit_is_private_and_commit_consumes_original_scalar_range_once(self):
        self.begin()
        self.composition.stage(self.request('update', text='字😀e\u0301', selection=[1, 4]), dict(self.context))
        self.assertEqual(self.context['content'], 'Aé😀e\u0301Z')
        self.assertEqual(self.composition.diagnostic()['preedit'], '字😀e\u0301')
        edit = self.composition.stage(self.request('commit', text='字😀'), dict(self.context))
        self.assertEqual(edit, {'start': 1, 'end': 4, 'text': '字😀'})
        self.assertIsNone(self.composition.active)
        with self.assertRaisesRegex(SourceRejected, 'stale-native-composition'):
            self.composition.stage(self.request('commit', text='字😀'), dict(self.context))

    def test_full_record_clock_cancels_selection_aba_and_focus_context(self):
        for field in ('text_record_version', 'focus_generation'):
            self.begin()
            self.context[field] += 2  # content and final selection are unchanged
            self.composition.poll()
            self.assertIsNone(self.composition.active)
            with self.assertRaises(SourceRejected):
                self.composition.stage(self.request('commit', text='late'), dict(self.context))

    def test_revocation_fence_and_expiry_cancel_before_late_commit(self):
        for cause in ('fence', 'expiry'):
            self.generation = 7
            self.guard['deadline_ns'] = time.monotonic_ns() + 5_000_000_000
            self.begin()
            if cause == 'fence':
                self.generation = 0
            else:
                self.composition.active['guard']['deadline_ns'] = 0
            self.composition.poll()
            self.assertIsNone(self.composition.active)
            with self.assertRaises(SourceRejected):
                self.composition.stage(self.request('commit', text='late'), dict(self.context))

    def test_source_removal_or_observation_allocation_failure_cancels(self):
        for failure in (SourceRejected('retired-source-object'), MemoryError()):
            self.begin()
            def unavailable(_):
                raise failure
            self.composition.read_context = unavailable
            self.composition.poll()
            self.assertIsNone(self.composition.active)
            self.composition.read_context = lambda _: dict(self.context)

    def test_byte_and_scalar_limits_check_before_transient_change(self):
        self.begin()
        for text, selection in (('😀', [0, 2]), ('x' * (MAX_TEXT_BYTES + 1), [0, 0]), ('\0', [0, 0])):
            with self.assertRaises(SourceRejected):
                self.composition.stage(self.request('update', text=text, selection=selection), dict(self.context))
            self.assertEqual(self.composition.active['preedit'], '')
        with self.assertRaisesRegex(SourceRejected, 'already-active'):
            self.begin()

    def test_native_control_renewal_extends_held_preedit_without_reviving_cancelled_context(self):
        dispatcher = NativeDispatcher.__new__(NativeDispatcher)
        dispatcher.composition, dispatcher.promised_artifact = self.composition, None
        dispatcher.control, dispatcher.reply = ('lease', 7, 20), lambda *_: None
        self.guard['deadline_ns'] = 20
        with patch('adapters.gtk4.composition.time.monotonic_ns', return_value=10):
            self.begin()
        dispatcher.handle({'request_id': 1, 'kind': 'control', 'lease_id': 'lease', 'generation': 7, 'deadline_ns': 30})
        with patch('adapters.gtk4.composition.time.monotonic_ns', return_value=25):
            self.composition.poll()
            self.assertIsNotNone(self.composition.active)
        with patch('adapters.gtk4.composition.time.monotonic_ns', return_value=30):
            self.composition.poll()
            self.assertIsNone(self.composition.active)
        dispatcher.handle({'request_id': 2, 'kind': 'control', 'lease_id': 'lease', 'generation': 7, 'deadline_ns': 60})
        self.assertIsNone(self.composition.active)


if __name__ == '__main__':
    unittest.main()
