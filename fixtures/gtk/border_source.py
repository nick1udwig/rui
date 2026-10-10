#!/usr/bin/env python3
"""Source-owned square border frames: differing paint and clipped transforms."""
import argparse
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
import gi
for name in ('Gtk', 'Gsk', 'Gdk', 'Graphene'): gi.require_version(name, '4.0' if name != 'Graphene' else '1.0')
from gi.repository import Gtk, Gsk, Gdk, Graphene, GLib
from adapters.gtk4.capture import CaptureWidget
from adapters.sdk.producer import CoherentSource


def paint(value):
    result = Gdk.RGBA(); result.parse(value); return result


class BorderPanel(Gtk.Widget):
    def __init__(self):
        super().__init__(); self.case = 0; self.set_size_request(240,180)

    def do_snapshot(self, snapshot):
        snapshot.append_color(paint('#ffffff'), Graphene.Rect().init(0,0,240,180))
        # An opaque underlay proves that the center remains unpainted.
        snapshot.append_color(paint('#dddddd'), Graphene.Rect().init(5,5,230,170))
        bounds = Graphene.Rect().init(24,26,120,90)
        outline = Gsk.RoundedRect(); outline.init_from_rect(bounds,0)
        widths = [6,6,6,6] if self.case == 0 else [9,9,9,9]
        node = Gsk.BorderNode.new(outline,widths,[paint(c) for c in ('#ff0000','#008800','#0000ff','#111111')])
        if self.case >= 2:
            transform = Gsk.Transform.new().translate(Graphene.Point().init(12,8))
            node = Gsk.TransformNode.new(node,transform)
        if self.case == 3:
            node = Gsk.ClipNode.new(node,Graphene.Rect().init(60,30,120,90))
        snapshot.append_node(node)


def main():
    parser=argparse.ArgumentParser(); parser.add_argument('--output',type=Path,required=True); args=parser.parse_args()
    app=Gtk.Application(application_id='org.rui.BorderCoverage')
    def activate(app):
        source=CoherentSource(); source.register('root',1,lambda:{'label':'Square border coverage'})
        panel=BorderPanel(); seen=set()
        def captured(capture,frame):
            if panel.case in seen or not frame['mapped']: return
            if frame['gaps']: raise RuntimeError(str(frame['gaps']))
            seen.add(panel.case)
            def advance():
                if len(seen)==4: app.quit()
                else: panel.case+=1; source.changed(); panel.queue_draw()
                return False
            GLib.timeout_add(40,advance)
        capture=CaptureWidget(source,panel,'border','root',args.output,captured)
        window=Gtk.ApplicationWindow(application=app); window.set_child(capture); window.present()
        GLib.timeout_add_seconds(8,lambda: (_ for _ in ()).throw(RuntimeError('border deadline')))
    app.connect('activate',activate); app.run([])


if __name__=='__main__': main()
