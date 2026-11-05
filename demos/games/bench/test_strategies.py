import struct
import unittest

import strategies


class StrategyValidation(unittest.TestCase):
    def test_publication_boundaries_and_truncation(self):
        self.assertEqual(strategies.publications(b"\0\0\0\3abc\0\0\0\1d"), [b"abc", b"d"])
        for data in (b"", b"\0\0", b"\0\0\0\3ab", b"\0\0\0\0"):
            with self.assertRaises(ValueError):
                strategies.publications(data)

    def test_each_compressed_update_decodes_without_future_frames(self):
        frames = [b"a" * 40000, b"a" * 39999 + b"b", b"", bytes(range(256)) * 300, b"new epoch"]
        for codec in ("zlib", "zstd"):
            for reset in (0, 1, 2):
                result = strategies.compress_frames(frames, codec, 3, reset, repeats=2)
                self.assertTrue(result["exact_incremental_round_trip"])
                self.assertEqual(len(result["frame_bytes"]), len(frames))

    def test_beam_growth_removal_reordering_and_reset(self):
        a = [1.2345, -4.0001, 100.1, 200, 0.123456, 0, 1]
        b = [-100, 101, 0.9999, 0, 0.01, 0.1, 0.12345]
        frames = [{"lines": lines} for lines in ([a], [a, b], [b, a], [], [b], [a])]
        for style in ("f64", "decimal-delta", "byte-shuffle"):
            packed, decoded = strategies.beam_arrays(frames, style, reset=2)
            self.assertEqual(decoded, frames)
            self.assertEqual([struct.unpack_from("<I", frame)[0] for frame in packed], [1, 2, 2, 0, 1, 1])

    def test_decimal_codec_rejects_precision_loss(self):
        for value in (0.123456789, -0.0):
            with self.assertRaisesRegex(ValueError, "exactly representable"):
                strategies.beam_arrays([{"lines": [[value, 0, 0, 0, 0, 0, 0]]}], "decimal-delta", 25)
        with self.assertRaisesRegex(ValueError, "truncated"):
            strategies.read_varints(b"\x80")


if __name__ == "__main__":
    unittest.main()
