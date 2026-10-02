import unittest

import gi
for name, version in [('Gtk', '4.0'), ('Gsk', '4.0'), ('Graphene', '1.0')]:
    gi.require_version(name, version)
from gi.repository import Gsk, Gdk, Graphene

from adapters.gtk4.capture import Exporter, census
from adapters.sdk.producer import CoherentSource


class CaptureBoundsTests(unittest.TestCase):
    def test_supported_deep_native_tree_is_classified_before_python_recursion(self):
        paint = Gdk.RGBA()
        paint.parse('#ffffff')
        bounds = Graphene.Rect().init(0, 0, 10, 10)
        node = Gsk.ColorNode.new(paint, bounds)
        for _ in range(180):
            node = Gsk.ClipNode.new(node, bounds)
        exporter = Exporter(CoherentSource(), 'deep', 1)
        exporter.export(node)
        self.assertEqual([gap['reason'] for gap in exporter.gaps], ['source-depth-ceiling'])
        self.assertEqual(exporter.depth, 0)
        counts, regions = census(node)
        self.assertEqual(sum(counts.values()), 128)
        self.assertEqual(len(exporter.scene), 128)
        self.assertEqual(regions[-1]['truncated'], 'source-depth-ceiling')


if __name__ == '__main__':
    unittest.main()
