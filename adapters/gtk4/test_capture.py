import unittest
import tempfile
import time
from pathlib import Path

import gi
for name, version in [('Gtk', '4.0'), ('Gsk', '4.0'), ('Graphene', '1.0')]:
    gi.require_version(name, version)
from gi.repository import Gsk, Gdk, Graphene

from adapters.gtk4.capture import CaptureMixin, Exporter, census
from adapters.sdk.producer import CoherentSource
from adapters.gtk4.state import NativeRegistry, SourceRejected
from types import SimpleNamespace


class CaptureBoundsTests(unittest.TestCase):
    def capture_fixture(self, source, directory, callback=None):
        class Capture(CaptureMixin):
            def get_width(self):
                return 10

            def get_height(self):
                return 10

        capture = Capture()
        capture.setup_capture(source, 'view', 'root', Path(directory), callback, 1)
        return capture

    def submit(self, capture):
        paint = Gdk.RGBA()
        paint.parse('#ffffff')
        node = Gsk.ColorNode.new(paint, Graphene.Rect().init(0, 0, 10, 10))
        appended = []
        capture.capture_node(SimpleNamespace(append_node=appended.append), node, capture.source_boundary(), time.monotonic_ns())
        self.assertEqual(appended, [node])

    def test_unmap_remap_preserves_objects_destroy_recreate_issues_fresh_view(self):
        source = CoherentSource()
        source.register('root', 1, lambda: {})
        with tempfile.TemporaryDirectory() as directory:
            capture = self.capture_fixture(source, directory)
            self.submit(capture)
            first = capture.last_frame
            capture.publish_lifecycle()
            self.submit(capture)
            remapped = capture.last_frame
            for field in ('surface_id', 'semantic_root'):
                self.assertEqual(first[field], remapped[field])
            self.assertEqual(first['session']['id'], remapped['session']['id'])
            self.assertNotEqual(first['scene_root'], remapped['scene_root'])
            capture.retire_source()
            self.assertFalse(capture.source_alive)
            self.assertIn(first['surface_id'], source.retired_object_ids)
            self.assertIn(first['session']['id'], source.retired_object_ids)
            source.register('root', 1, lambda: {})
            recreated = self.capture_fixture(source, directory)
            self.submit(recreated)
            for field in ('surface_id', 'semantic_root'):
                self.assertNotEqual(first[field], recreated.last_frame[field])
            self.assertNotEqual(first['session']['id'], recreated.last_frame['session']['id'])
            self.assertEqual(recreated.last_frame['revision'], 1)
            self.assertNotEqual(first['routing_token'], recreated.last_frame['routing_token'])
            with self.assertRaisesRegex(RuntimeError, 'retired source surface'):
                capture.source_boundary()

    def test_reentrant_capture_observer_commits_unmap_and_destroy_in_order(self):
        for destroyed in (False, True):
            source = CoherentSource()
            source.register('root', 1, lambda: {})
            observed = []
            def callback(capture, frame):
                self.assertIs(capture.last_frame, frame)
                observed.append((frame['revision'], frame['mapped']))
                if frame['mapped']:
                    capture.publish_lifecycle(destroyed=destroyed)
            with tempfile.TemporaryDirectory() as directory:
                capture = self.capture_fixture(source, directory, callback)
                self.submit(capture)
                self.assertEqual(observed, [(1, True), (2, False)])
                self.assertEqual(capture.last_frame['revision'], 2)
                self.assertFalse(capture.last_frame['mapped'])
                self.assertEqual(capture.source_alive, not destroyed)
                self.assertEqual(capture.last_frame['capture_boundary'], 'source-lifecycle-signal')

    def test_disposal_before_first_snapshot_retires_registered_objects(self):
        source = CoherentSource()
        source.register('root', 1, lambda: {})
        semantic = source.identifier('semantic:root')
        surface = source.identifier('surface:view')
        with tempfile.TemporaryDirectory() as directory:
            capture = self.capture_fixture(source, directory)
            capture.retire_source()
            self.assertFalse(capture.source_alive)
            self.assertIn(semantic, source.retired_semantic_ids)
            self.assertIn(surface, source.retired_object_ids)
            self.assertEqual(list(Path(directory).iterdir()), [])

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

    def test_late_source_incarnation_surface_and_destroyed_object_guards(self):
        source = CoherentSource()
        source.register('button', 3, lambda: {})
        target = source.identifier('semantic:button')
        surface = source.identifier('surface:view')
        widget = SimpleNamespace(get_mapped=lambda: True, get_sensitive=lambda: True)
        capture = SimpleNamespace(source_alive=True, source_mapped=True, last_frame={'routing_token': '01' * 16})
        registry = NativeRegistry.__new__(NativeRegistry)
        registry.source = source
        registry.widgets = {'button': (widget, 'view')}
        registry.surfaces = {'view': capture}
        self.assertEqual(registry.resolve_event(source.identity, surface, target, source.source_revision, '01' * 16), ('button', widget))
        with self.assertRaisesRegex(SourceRejected, 'retired-source-incarnation'):
            registry.resolve_event(CoherentSource().identity, surface, target, source.source_revision, '01' * 16)
        capture.source_mapped = False
        with self.assertRaisesRegex(SourceRejected, 'unmapped-or-retired'):
            registry.resolve_event(source.identity, surface, target, source.source_revision, '01' * 16)
        capture.source_mapped = True
        source.unregister('button')
        source.register('button', 3, lambda: {})
        with self.assertRaisesRegex(SourceRejected, 'retired-source-object'):
            registry.resolve_event(source.identity, surface, target, source.source_revision, '01' * 16)
        self.assertNotEqual(target, source.identifier('semantic:button'))


if __name__ == '__main__':
    unittest.main()
