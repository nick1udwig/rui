import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from adapters.sdk.producer import path_resource
from fixtures.gtk.compare import native_glyph_geometry, pixels


class PixelComparisonTests(unittest.TestCase):
    def test_measurement_and_visual_report_match_all_exact_golden_envelopes(self):
        fixture = Path(__file__).resolve().parent
        measurement = json.loads((fixture / 'measurement.json').read_text())
        strict = next(run for run in measurement['runs'] if run['theme'] == 'strict')
        compared = json.loads((fixture / 'pixel-comparison.json').read_text())
        self.assertEqual(len(strict['captures']), 20)
        self.assertEqual(len(compared['captures']), 17)
        for capture in strict['captures']:
            label = f"{capture['surface']}-{capture['revision']:04d}"
            digest = hashlib.sha256((fixture / 'golden' / (label + '.bin')).read_bytes()).hexdigest()
            manifest = json.loads((fixture / 'golden' / (label + '.json')).read_text())
            self.assertEqual(capture['binding'], manifest['binding'])
            self.assertEqual(digest, capture['envelope_sha256'])
            self.assertEqual(digest, manifest['envelope_sha256'])
            if capture['mapped']:
                pixels = next(result for result in compared['captures'] if result['capture'] == label + '.json')
                self.assertEqual(pixels['envelope_sha256'], digest)
                self.assertEqual(pixels['local_reference_sha256'], manifest['local_reference']['sha256'])
                self.assertTrue(pixels['pixels']['passed'])

    def test_straight_pam_conversion_and_wrong_flat_interior_rejection(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            native = bytes([0, 0, 64, 128]) * 16
            (directory / 'surface-0001.local.bgra').write_bytes(native)
            manifest = {'surface_key': 'surface', 'revision': 1, 'local_reference': {'width': 4, 'height': 4, 'stride': 16, 'sha256': hashlib.sha256(native).hexdigest()}}
            header = b'P7\nWIDTH 4\nHEIGHT 4\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n'
            pam = directory / 'replay.pam'
            pam.write_bytes(header + bytes([128, 0, 0, 128]) * 16)
            self.assertTrue(pixels(manifest, directory, pam)['passed'])
            pam.write_bytes(header + bytes([128, 20, 0, 128]) + bytes([128, 0, 0, 128]) * 15)
            result = pixels(manifest, directory, pam)
            self.assertFalse(result['passed'])
            self.assertEqual(result['interior_bad_pixels'], 1)

    def test_outline_geometry_is_checked_independently_of_pixel_tolerance(self):
        resource = path_resource([(1, (0, 0)), (2, (1, 0)), (2, (1, 1)), (5, ())], glyph=True)
        glyph = {'resource': resource['digest'], 'font_size': 10, 'path_bounds_em': [0, 0, 1, 1]}
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / (resource['digest'] + '.rvp')).write_bytes(resource['body'])
            self.assertEqual(native_glyph_geometry({'source_glyph_evidence': [glyph]}, directory)['glyphs_checked'], 1)
            glyph['path_bounds_em'] = [0, 0, 0.9, 1]
            with self.assertRaises(ValueError):
                native_glyph_geometry({'source_glyph_evidence': [glyph]}, directory)


if __name__ == '__main__':
    unittest.main()
