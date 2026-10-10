import unittest
import tempfile
import time
from unittest.mock import patch
from pathlib import Path

import gi
for name, version in [('Gtk', '4.0'), ('Gsk', '4.0'), ('Graphene', '1.0')]:
    gi.require_version(name, version)
from gi.repository import Gsk, Gdk, Graphene, Gtk

from adapters.gtk4.capture import CaptureMixin, Exporter, census
from adapters.sdk.producer import CoherentSource, MAX_RECORDS
from adapters.gtk4.state import NativeRegistry, SourceRejected
from types import SimpleNamespace


class CaptureBoundsTests(unittest.TestCase):
    def test_square_border_paths_are_bounded_and_inexact_variants_decline(self):
        colors = []
        for value in ('#ff0000', '#008800', '#0000ff', '#111111'):
            paint = Gdk.RGBA(); paint.parse(value); colors.append(paint)
        bounds = Graphene.Rect().init(3, 5, 100, 80)
        for rounding, widths, expected in (
                (0, [6]*4, None),
                (0, [9, 5, 12, 3], 'nonuniform-border-widths'),
                (4, [6]*4, 'nonuniform-rounded-border'),
                (0, [41]*4, 'border-width-not-inward-profile')):
            exporter = Exporter(CoherentSource(), 'border', 1)
            outline = Gsk.RoundedRect(); outline.init_from_rect(bounds, rounding)
            node = Gsk.BorderNode.new(outline, widths, colors)
            exporter.export(node)
            if expected:
                self.assertEqual([g['reason'] for g in exporter.gaps], [expected])
                self.assertEqual(exporter.resources, set())
            else:
                self.assertEqual(exporter.gaps, [])
                self.assertEqual(len(exporter.resources), 4)
                self.assertEqual(exporter.extractions['gsk-square-border'], 4)
                self.assertEqual(sum(r['primitive']['type'] == 'FillPath' for r in exporter.scene), 4)
        colors[0].alpha = .5
        exporter = Exporter(CoherentSource(), 'border', 1)
        outline = Gsk.RoundedRect(); outline.init_from_rect(bounds, 0)
        exporter.export(Gsk.BorderNode.new(outline, [5]*4, colors))
        self.assertEqual([g['reason'] for g in exporter.gaps], ['nonopaque-srgb-composition'])
        self.assertEqual(exporter.resources, set())

    def test_multicolor_border_scale_is_explicitly_partial(self):
        colors = []
        for value in ('#ff0000', '#008800', '#0000ff', '#111111'):
            paint = Gdk.RGBA(); paint.parse(value); colors.append(paint)
        outline = Gsk.RoundedRect(); outline.init_from_rect(Graphene.Rect().init(3,5,100,80),0)
        border = Gsk.BorderNode.new(outline,[6]*4,colors)
        transform = Gsk.Transform.new().scale(1.25,1.25)
        exporter = Exporter(CoherentSource(),'scaled-border',1)
        exporter.export(Gsk.TransformNode.new(border,transform))
        self.assertEqual([g['reason'] for g in exporter.gaps], ['nonuniform-border-not-pixel-aligned'])
        self.assertEqual(exporter.resources, set())
        self.assertEqual(exporter.effective_transform, [1,0,0,1,0,0])

    def test_refused_binding_registration_leaves_registry_and_identity_state_unchanged(self):
        source = CoherentSource()
        source.register('button', 3, lambda: {}, actions=['Activate'])
        for index in range(MAX_RECORDS - len(source._ids)):
            source.identifier('quota:' + str(index))
        registry = NativeRegistry.__new__(NativeRegistry)
        registry.source, registry.widgets = source, {}
        # Registration fails before touching a native object or its signals.
        # A small type witness lets the quota path run without a display.
        class WidgetWitness:
            pass
        with patch.object(Gtk, 'Widget', WidgetWitness):
            with self.assertRaisesRegex(ValueError, 'identity ceiling'):
                registry.bind('button', WidgetWitness(), 'view')
        self.assertEqual(registry.widgets, {})
        self.assertNotIn('binding:button', source._ids)
        self.assertEqual(len(source._ids), MAX_RECORDS)

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

    def test_gtk_caret_opacity_endpoints_are_exact_and_partial_opacity_still_declines(self):
        paint = Gdk.RGBA()
        paint.parse('#111111')
        bounds = Graphene.Rect().init(0, 0, 1, 16)
        child = Gsk.ColorNode.new(paint, bounds)
        for opacity in (0.0, 0.5, 1.0):
            exporter = Exporter(CoherentSource(), 'caret', 1)
            node = Gsk.OpacityNode.new(child, opacity)
            root = exporter.export(node)
            record = next(record for record in exporter.scene if record['id'] == root)
            self.assertEqual(record['primitive']['type'], 'Group')
            if opacity == 0.5:
                self.assertEqual([gap['reason'] for gap in exporter.gaps], ['unsupported-opacity-node'])
            else:
                self.assertEqual(exporter.gaps, [])
                self.assertEqual(len(record['children']), int(opacity))
                self.assertTrue(all(record['opacity'] == 1 for record in exporter.scene))

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
        widget = SimpleNamespace(get_mapped=lambda: True, is_sensitive=lambda: True)
        capture = SimpleNamespace(source_alive=True, source_mapped=True, last_frame={'routing_token': '01' * 16})
        registry = NativeRegistry.__new__(NativeRegistry)
        registry.source = source
        registry.widgets = {'button': (widget, 'view')}
        registry.surfaces = {'view': capture}
        self.assertEqual(registry.resolve_event(source.identity, surface, target, source.source_revision, '01' * 16), ('button', widget))
        source.changed()
        self.assertEqual(registry.resolve_semantic(source.identity, surface, target), ('button', widget))
        with self.assertRaisesRegex(SourceRejected, 'stale-source-context'):
            registry.resolve_event(source.identity, surface, target, source.source_revision - 1, '01' * 16)
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
