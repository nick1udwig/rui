#!/usr/bin/env python3
"""Actual GTK widget geometry/lifecycle negatives; no remote action claim."""
import json
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
import gi
gi.require_version('Gtk', '4.0')
from gi.repository import Gtk, GLib, Gsk, Graphene, Gdk
from adapters.gtk4.capture import CaptureWidget
from adapters.gtk4.state import NativeRegistry
from adapters.sdk.producer import CoherentSource
from fixtures.gtk.corpus import STRICT_CSS


class Probe:
    def __init__(self, app, output):
        self.app, self.output = app, output
        output.mkdir(parents=True, exist_ok=True)
        self.source = CoherentSource()
        self.window = Gtk.ApplicationWindow(application=app)
        self.window.set_default_size(500, 300)
        provider = Gtk.CssProvider(); provider.load_from_data(STRICT_CSS)
        Gtk.StyleContext.add_provider_for_display(Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        self.fixed = Gtk.Fixed()
        self.button = Gtk.Button(label='Bound native control')
        self.fixed.put(self.button, 10, 10)
        self.clip = Gtk.Fixed(); self.clip.set_size_request(100, 100); self.clip.set_overflow(Gtk.Overflow.HIDDEN)
        self.fixed.put(self.clip, 220, 120)
        self.fixed.set_child_transform(self.clip, Gsk.Transform().rotate(90))
        self.counter = Gtk.Button(label='Counter rotated')
        self.clip.put(self.counter, 20, 20)
        self.clip.set_child_transform(self.counter, Gsk.Transform().rotate(-90))
        self.source.register('root', 1, lambda: {'label': 'Native binding source'})
        self.source.register('button', 3, lambda: {'label': 'Bound native control'}, parent='root', actions=['Activate'])
        self.source.register('counter', 3, lambda: {'label': 'Counter rotated'}, parent='root', actions=['Activate'])
        self.registry = NativeRegistry(self.source, self.window)
        self.registry.bind('button', self.button, 'binding-source')
        self.registry.bind('counter', self.counter, 'binding-source')
        # A native background remains submitted when both buttons are hidden
        # or clipped; a None GSK child would produce no visual capture.
        background = Gtk.Box()
        self.fixed.set_hexpand(True); self.fixed.set_vexpand(True)
        background.append(self.fixed)
        self.capture = CaptureWidget(self.source, background, 'binding-source', 'root', output, self.captured)
        self.registry.surface('binding-source', self.capture)
        self.window.set_child(self.capture)
        self.stage, self.waiting, self.frames = 0, True, []
        self.binding = self.source._ids['binding:button']
        self.window.present()
        GLib.timeout_add_seconds(6, self.deadline)

    def captured(self, capture, frame):
        if not self.waiting:
            return
        self.waiting = False
        regions = {r['semantic_id']: r['interaction_region'] for r in frame['binding_regions']}
        target = self.source._ids.get('semantic:button')
        ordinary = regions.get(target)
        if self.stage in (0, 1, 3):
            assert ordinary is not None
            assert abs(ordinary[0] - (40 if self.stage == 1 else 10)) < .001
            assert any(r['id'] == self.binding for r in frame['visual_bindings'])
        elif self.stage in (2, 4):
            assert ordinary is None
            assert any(r['id'] == self.binding for r in frame['visual_bindings'])
        else:
            assert all(r['id'] != self.binding for r in frame['visual_bindings'])
            assert self.binding in self.source.retired_object_ids
            assert ('binding', self.binding) not in self.source._models
        # The child's aggregate transform is positive axis-aligned, while its
        # clipping ancestor is rotated. Its bounding-box clip is not safe.
        assert self.registry.axis_aligned(self.counter, self.capture)
        assert not self.registry.axis_aligned(self.clip, self.capture)
        assert regions[self.source._ids['semantic:counter']] is None
        self.frames.append({'stage': ['initial', 'moved', 'hidden', 'remapped', 'disabled-ancestor', 'removed'][self.stage],
                            'capture': f'binding-source-{frame["revision"]:04d}.json',
                            'binding_id': self.binding, 'counter_rotated_clip_denied': True})
        if self.stage == 5:
            (self.output / 'binding-report.json').write_text(json.dumps({'scope': 'actual native local geometry/lifecycle, no remote command claim', 'gtk_version': [4, 22, 4], 'frames': self.frames}, indent=2))
            self.app.quit()
        else:
            GLib.timeout_add(50, self.next)

    def next(self):
        self.stage += 1
        if self.stage == 1:
            self.fixed.move(self.button, 40, 10)
        elif self.stage == 2:
            self.button.set_visible(False)
        elif self.stage == 3:
            self.fixed.move(self.button, 10, 10)
            self.button.set_visible(True)
        elif self.stage == 4:
            self.fixed.set_sensitive(False)
        else:
            self.fixed.set_sensitive(True)
            self.registry.retire('button')
            self.button.unparent()
        self.source.changed()
        self.waiting = True
        self.capture.queue_draw()
        return False

    def deadline(self):
        self.app.quit()
        return False


def main():
    output = Path(sys.argv[1])
    app = Gtk.Application(application_id='org.rui.NativeBindingProbe')
    app.connect('activate', lambda app: setattr(app, 'probe', Probe(app, output)))
    result = app.run(None)
    if not (output / 'binding-report.json').exists():
        raise ValueError('native binding scenario failed before all lifecycle proofs')
    return result


if __name__ == '__main__':
    raise SystemExit(main())
