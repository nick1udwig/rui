import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest
import numpy as np

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('codecs_bench', HERE/'codecs.py')
codecs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(codecs)


class RasterValidation(unittest.TestCase):
    def test_lossless_and_foreground_error_are_distinct(self):
        with tempfile.TemporaryDirectory() as temporary:
            a = Path(temporary)/'a.rgba'
            b = Path(temporary)/'b.rgba'
            pixels = np.zeros((1, 8, 8, 4), dtype=np.uint8)
            pixels[..., 3] = 255
            pixels[0, 3, 3, :3] = 120
            a.write_bytes(pixels.tobytes())
            b.write_bytes(pixels.tobytes())
            exact = codecs.quality(a, b, 8, 8, 1)
            self.assertTrue(exact['exact'])
            self.assertIsNone(exact['psnrDb'])
            pixels[0, 3, 3, 1] = 80
            b.write_bytes(pixels.tobytes())
            changed = codecs.quality(a, b, 8, 8, 1)
            self.assertFalse(changed['exact'])
            self.assertGreater(changed['foregroundRmse'], 0)
            self.assertGreater(changed['foregroundChannelsOver8Fraction'], 0)
            b.write_bytes(b'bad')
            with self.assertRaisesRegex(ValueError, 'frame count'):
                codecs.quality(a, b, 8, 8, 1)

    def test_png_parser_preserves_complete_frame_boundaries(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary)/'stream'
            frame = b'\x89PNG\r\n\x1a\n' + struct.pack('>I', 0) + b'IEND' + b'\0'*4
            path.write_bytes(frame*2)
            self.assertEqual([x['size'] for x in codecs.png_chunks(path)], [len(frame)]*2)
            path.write_bytes(frame[:-2])
            with self.assertRaisesRegex(ValueError, 'truncated'):
                codecs.png_chunks(path)
            path.write_bytes(b'bad')
            with self.assertRaisesRegex(ValueError, 'signature'):
                codecs.png_chunks(path)

    def test_h264_sps_becomes_the_actual_webcodecs_identifier(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary)/'stream.h264'
            path.write_bytes(b'\0\0\0\1\x67\x42\xc0\x1f' + b'\0'*8)
            self.assertEqual(codecs.avc_codec(path), 'avc1.42c01f')
            path.write_bytes(b'invalid')
            with self.assertRaisesRegex(ValueError, 'SPS'):
                codecs.avc_codec(path)


if __name__ == '__main__':
    unittest.main()
