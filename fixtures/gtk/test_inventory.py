import copy
import json
from pathlib import Path
import unittest
from fixtures.gtk.inventory import inventory

ROOT = Path(__file__).resolve().parents[2]


class CoverageInventory(unittest.TestCase):
    def inputs(self):
        return [json.loads((ROOT/p).read_text()) for p in ('fixtures/gtk/measurement.json', 'fixtures/gtk/negative-report.json')]

    def test_counts_are_occurrences_and_all_negatives_are_required(self):
        measurement, negatives = self.inputs()
        result = inventory(measurement, negatives)
        default = next(row for row in result['runs'] if row['style'] == 'default')
        self.assertEqual(default['gapOccurrences'], 104)
        self.assertLess(default['uniqueGapReasons'], default['gapOccurrences'])
        self.assertFalse(result['negativeCases']['native-vector-control'])
        for mutation in ('capture', 'raster', 'negative', 'pin'):
            m, n = copy.deepcopy(measurement), copy.deepcopy(negatives)
            if mutation == 'capture': m['runs'][0]['captures'][0]['capture_boundary'] = 'paintable'
            elif mutation == 'raster': n['fallback_probe']['forbidden_export_calls'] = {'draw': 1}
            elif mutation == 'negative': n['cases'][0]['gaps'] = []
            else: m['pin']['gtk'] = 'other'
            with self.subTest(mutation=mutation), self.assertRaises(ValueError): inventory(m,n)
