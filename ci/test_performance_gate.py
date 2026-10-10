import copy
import importlib.util
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('gate', Path(__file__).with_name('performance-gate.py'))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)
ROOT = Path(__file__).resolve().parents[1]


class PerformanceGate(unittest.TestCase):
    def reports(self):
        return [json.loads((ROOT/f'fixtures/performance/browser-p4-after/run-{i}.json').read_text()) for i in (1,2,3)]

    def test_retained_measurements_report_known_latency_miss_without_lowering_target(self):
        result = gate.evaluate(self.reports())
        self.assertFalse(result['passed'])
        self.assertTrue(all(not row['source']['passed'] for row in result['runs']))
        self.assertTrue(all(row['draw']['passed'] for row in result['runs']))

    def test_frozen_sample_profile_cannot_silently_skip_or_relabel_actions(self):
        for mutation in ('population', 'hardware', 'drop', 'correctness', 'negative', 'nan'):
            reports = self.reports()
            if mutation == 'population': reports[0]['actionSamples'].pop()
            elif mutation == 'hardware': reports[0]['hardware'] = 'other runner'
            elif mutation == 'drop': reports[0]['timing']['dropped'] = 1
            elif mutation == 'correctness': reports[0]['correctness']['passed'] = False
            elif mutation == 'negative': reports[0]['actionSamples'][0]['durationMs'] = -1
            else: reports[0]['actionSamples'][0]['durationMs'] = float('nan')
            with self.subTest(mutation=mutation), self.assertRaises(ValueError): gate.evaluate(reports)

    def test_stage_percentiles_are_not_added_instead_of_per_frame_durations(self):
        reports = self.reports()
        row = copy.deepcopy(reports[0]['timing']['samples'])
        draw = gate.draw_samples({'samples': row})
        self.assertGreaterEqual(len(draw),400)
        self.assertEqual(gate.samples([1]*200), {'p50':1,'p95':1,'p99':1})
        for entry in row:
            if entry['stage'] == 'composition': entry['revision'] = 'foreign'; break
        with self.assertRaises(ValueError): gate.draw_samples({'samples': row})


if __name__ == '__main__': unittest.main()
