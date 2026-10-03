import unittest
from collections import Counter

from adapters.gtk4.capture import Exporter
from adapters.sdk.producer import CoherentSource
from fixtures.gtk.fallback_probe import FallbackProbe
from fixtures.gtk.negative_source import CASES, PHOTO, memory_texture, source_node
from gi.repository import Gdk


class SourceNegativeTests(unittest.TestCase):
    def test_actual_native_nodes_declined_without_pixel_or_conversion_paths(self):
        photo = Gdk.Texture.new_from_filename(str(PHOTO))
        texture = memory_texture()
        reasons = ['unsupported-texture-node', 'unsupported-texture-node', 'unsupported-cairo-node', 'unsupported-blur-node', 'unsupported-gl-shader-node']
        with FallbackProbe() as probe:
            probe.prove_traps(texture)
            for case, reason in zip(CASES, reasons):
                source = CoherentSource()
                exporter = Exporter(source, 'negative', 1)
                exporter.export(source_node(case, photo, texture))
                self.assertEqual([gap['reason'] for gap in exporter.gaps], [reason])
                self.assertEqual([record['primitive']['type'] for record in exporter.scene], ['Group'])
                self.assertEqual(exporter.resources, set())
                self.assertEqual(source.resources, {})
                counts = exporter.counters()
                self.assertEqual(counts['source_nodes_visited'], 1)
                self.assertEqual(counts['node_decisions'], {'declined': 1})
                self.assertEqual(counts['geometry_extractions'], {})
                self.assertEqual(counts['local_reference_draws'], 0)
            exporter = Exporter(CoherentSource(), 'positive', 1)
            exporter.export(source_node(CASES[-1], photo, texture))
            self.assertEqual(exporter.gaps, [])
            self.assertEqual(exporter.extractions, Counter({'gsk-fill-path': 1}))
            self.assertEqual(len(exporter.resources), 1)
            self.assertEqual(probe.attempts, {})
            self.assertEqual(probe.controls, {'Texture.download': 1, 'ImageSurface': 1})


if __name__ == '__main__':
    unittest.main()
