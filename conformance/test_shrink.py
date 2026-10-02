import unittest

from conformance.shrink import shrink_transcript


class ShrinkTests(unittest.TestCase):
    def test_preserves_exact_dependencies_and_removes_irrelevant_history(self):
        entries = [{'kind': 'snapshot', 'hex': '0801'}]
        entries += [{'kind': 'transaction', 'hex': f'{index:04x}'} for index in range(64)]
        dependencies = [entries[0], entries[12], entries[57]]

        def failure(candidate):
            return all(entry in candidate for entry in dependencies)

        minimized, runs = shrink_transcript(entries, failure)
        self.assertEqual(minimized, dependencies)
        self.assertLess(runs, 512)
        self.assertEqual(len(entries), 65)
        self.assertTrue(all(not failure(minimized[:i] + minimized[i + 1:]) for i in range(3)))

    def test_budget_and_nonfailure_are_explicit(self):
        calls = []

        def failure(candidate):
            calls.append(candidate)
            return 7 in candidate

        result, runs = shrink_transcript(list(range(20)), failure, maximum_runs=3)
        self.assertEqual(len(calls), runs)
        self.assertLessEqual(runs, 3)
        self.assertIn(7, result)
        with self.assertRaises(ValueError):
            shrink_transcript([], failure)


if __name__ == '__main__':
    unittest.main()
