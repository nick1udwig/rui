import hashlib
from pathlib import Path
import tempfile
import unittest

from adapters.sdk.producer import path_resource
from fixtures.gtk.compare import native_glyph_geometry, pixels


class PixelComparisonTests(unittest.TestCase):
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
