import unittest
from adapters.sdk.timing import Timings


class TimingTests(unittest.TestCase):
    def test_overflow_and_sanitized_shape(self):
        timings = Timings(2)
        for revision in range(5):
            timings.record('dirty', revision)
        report = timings.report()
        self.assertEqual(report['dropped'], 3)
        self.assertEqual([s['revision'] for s in report['samples']], ['3', '4'])
        self.assertEqual(set(report['samples'][0]), {'stage', 'revision', 'durationMs', 'count'})
        with self.assertRaises(ValueError):
            Timings(16385)
