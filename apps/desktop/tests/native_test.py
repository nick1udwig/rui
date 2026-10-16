"""Exercise the boundary between read-only validation and native mutation."""
import time
import unittest
from types import SimpleNamespace
from unittest.mock import Mock
from apps.desktop.native import DesktopReader


class DispatchBoundaryTest(unittest.TestCase):
    def setUp(self):
        self.reader = object.__new__(DesktopReader)
        self.reader.GLib = SimpleNamespace(Variant=lambda *args: args)
        self.node = dict(key=(':1.2', '/button'), fingerprint='fresh',
                         actions=['Activate'], disabled=False)
        self.reader.sample = Mock(return_value={'nodes': [self.node]})
        self.reader.call = Mock(return_value=(True,))
        self.request = dict(op='dispatch', source={}, key=[':1.2', '/button'],
                            fingerprint='fresh', action='Activate', expires=(time.time() + 5) * 1000)

    def test_failed_observation_is_rejected_without_dispatch(self):
        for error in (TimeoutError(), ValueError('children changed')):
            with self.subTest(error=error):
                self.reader.sample.side_effect = error
                self.assertEqual(self.reader.execute(self.request)['status'], 'rejected')
                self.reader.call.assert_not_called()

    def test_stale_target_is_rejected_without_dispatch(self):
        self.request['fingerprint'] = 'old'
        self.assertEqual(self.reader.execute(self.request)['status'], 'rejected')
        self.reader.call.assert_not_called()

    def test_timeout_after_attempt_is_unknown_and_never_retried(self):
        self.reader.call.side_effect = TimeoutError()
        self.assertEqual(self.reader.execute(self.request), {'status': 'unknown'})
        self.reader.call.assert_called_once()

    def test_post_dispatch_observation_failure_preserves_known_result(self):
        self.reader.sample.side_effect = [{'nodes': [self.node]}, TimeoutError()]
        self.assertEqual(self.reader.execute(self.request), {'status': 'dispatched'})
        self.reader.call.assert_called_once()


if __name__ == '__main__':
    unittest.main()
