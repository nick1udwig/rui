#!/usr/bin/env python3
"""Real submitted native GSK negatives; original raster input stays local."""
from pathlib import Path
import argparse
import hashlib
import json
import sys
import warnings

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
import gi
for name, version in [('Gtk', '4.0'), ('Gsk', '4.0'), ('Gdk', '4.0'), ('Graphene', '1.0')]:
    gi.require_version(name, version)
from gi.repository import Gtk, Gsk, Gdk, Graphene, GLib, Gio
from adapters.gtk4.capture import CaptureWidget
from adapters.sdk.producer import CoherentSource
from fixtures.gtk.fallback_probe import FallbackProbe


PHOTO = Path(__file__).parent / 'assets/PIA19400.jpg'
PHOTO_SHA256 = 'cfb90122fb690c0661002606310bd20d5b236f2b628994494aa23503a2ad50be'
CASES = ['original-photo', 'memory-texture', 'cairo-region', 'blur-effect', 'shader-node', 'native-vector-control']


def bounds():
    return Graphene.Rect().init(20, 20, 160, 100)


def paint(name):
    value = Gdk.RGBA()
    value.parse(name)
    return value


def memory_texture():
    # Independent synthetic checkerboard, never mislabeled a photograph.
    return Gdk.MemoryTexture.new(2, 2, Gdk.MemoryFormat.R8G8B8A8, GLib.Bytes.new(bytes([255, 0, 0, 255, 0, 0, 255, 255] * 2)), 8)


def source_node(case, photo, texture):
    if case == 'original-photo':
        return Gsk.TextureNode.new(photo, bounds())
    if case == 'memory-texture':
        return Gsk.TextureNode.new(texture, bounds())
    if case == 'cairo-region':
        node = Gsk.CairoNode.new(bounds())
        context = node.get_draw_context()
        context.set_source_rgb(.2, .7, .3)
        context.move_to(20, 20)
        context.curve_to(180, 0, 0, 120, 180, 120)
        context.line_to(20, 120)
        context.close_path()
        context.fill()
        del context  # complete source-native recording before submission
        return node
    if case == 'blur-effect':
        return Gsk.BlurNode.new(Gsk.ColorNode.new(paint('#336699'), bounds()), 6)
    if case == 'shader-node':
        # Native Cairo renders this unsupported shader as GTK's pink placeholder.
        # We claim actual captured node rejection, not successful GLSL execution.
        with warnings.catch_warnings():
            warnings.simplefilter('ignore', DeprecationWarning)
            shader = Gsk.GLShader.new_from_bytes(GLib.Bytes.new(b'void mainImage(out vec4 fragColor, in vec2 fragCoord, in vec2 resolution, in vec2 uv) { fragColor = vec4(uv, 0.0, 1.0); }'))
            return Gsk.GLShaderNode.new(shader, bounds(), GLib.Bytes.new(b''), [])
    if case == 'native-vector-control':
        builder = Gsk.PathBuilder.new()
        builder.move_to(20, 100)
        builder.cubic_to(30, 10, 130, 10, 180, 100)
        builder.close()
        return Gsk.FillNode.new(Gsk.ColorNode.new(paint('#336699'), bounds()), builder.to_path(), Gsk.FillRule.WINDING)
    raise AssertionError(case)


class Panel(Gtk.Widget):
    def __init__(self, photo, texture):
        super().__init__()
        self.photo, self.texture = photo, texture
        self.case = CASES[0]
        self.set_size_request(200, 140)

    def do_snapshot(self, snapshot):
        snapshot.append_color(paint('#ffffff'), Graphene.Rect().init(0, 0, self.get_width(), self.get_height()))
        snapshot.append_node(source_node(self.case, self.photo, self.texture))


class NegativeCorpus:
    def __init__(self, app, output, probe):
        self.app, self.output, self.probe = app, output, probe
        self.source = CoherentSource()
        self.cases = []
        self.seen = set()
        self.photo = Gdk.Texture.new_from_filename(str(PHOTO))
        self.texture = memory_texture()
        probe.prove_traps(self.texture)
        self.panel = Panel(self.photo, self.texture)
        self.source.register('negative-root', 1, lambda: {'label': 'Explicit no-fallback negative corpus'})
        self.source.register('source-region', 9, lambda: {'label': self.panel.case}, parent='negative-root')
        self.capture = CaptureWidget(self.source, self.panel, 'negative-content', 'negative-root', output, self.captured)
        self.capture.export_probe = probe.report
        self.window = Gtk.ApplicationWindow(application=app, title='Isolated RUI source negatives')
        self.window.set_child(self.capture)
        self.window.present()
        GLib.timeout_add_seconds(10, self.deadline)

    def captured(self, capture, frame):
        case = self.panel.case
        if case in self.seen or not frame['mapped']:
            return
        self.seen.add(case)
        label = f"{frame['surface_key']}-{frame['revision']:04d}"
        raw = (self.output / (label + '.local.bgra')).read_bytes()
        stride = frame['local_reference']['stride']
        pixels = [raw[y * stride + x * 4:y * stride + x * 4 + 4] for y in range(20, 120) for x in range(20, 180)]
        diagnostic = {'scope': 'local-reference-only-never-wire', 'distinct_region_colors': len(set(pixels)),
                      'nonwhite_region_pixels': sum(pixel != b'\xff\xff\xff\xff' for pixel in pixels),
                      'sha256': frame['local_reference']['sha256']}
        if case == 'original-photo' and diagnostic['distinct_region_colors'] <= 256:
            raise AssertionError('original photographic source was not drawn locally')
        self.cases.append({'case': case, 'label': f"{frame['surface_key']}-{frame['revision']:04d}",
                           'capture_token': frame['capture_token'], 'gaps': frame['gaps'],
                           'exporter_counters': frame['exporter_counters'], 'probe': self.probe.report(),
                           'local_reference': diagnostic})
        GLib.timeout_add(80, self.advance)

    def advance(self):
        index = CASES.index(self.panel.case) + 1
        if index == len(CASES):
            self.finish()
        else:
            self.panel.case = CASES[index]
            self.source.changed()
            self.panel.queue_draw()
        return False

    def finish(self):
        if self.probe.attempts:
            raise AssertionError('exporter attempted forbidden fallback')
        report = {'scope': 'actual-snapshot-vfunc-same-node-append', 'cases': self.cases,
                  'pin': {'gtk': '4.22.4', 'cairo': '1.18.4', 'local_backend': 'cairo'},
                  'source_photo': {'file': 'assets/PIA19400.jpg', 'bytes': PHOTO.stat().st_size,
                      'native_texture_size': [self.photo.get_width(), self.photo.get_height()],
                      'sha256': PHOTO_SHA256, 'credit': 'NASA/JPL-Caltech/MSSS/Texas A&M Univ.',
                      'source': 'https://science.nasa.gov/photojournal/sunset-in-mars-gale-crater/'},
                  'shader_scope': 'actual-node-declined-native-Cairo-pink-placeholder-no-GLSL-execution-claim',
                  'fallback_probe': self.probe.report()}
        hashes = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(self.output.iterdir()) if path.suffix in ('.bin', '.json', '.rvp')}
        report['retained_trace_sha256'] = hashes
        (self.output / 'negative-report.json').write_text(json.dumps(report, indent=2, sort_keys=True) + '\n')
        self.app.quit()

    def deadline(self):
        if len(self.seen) != len(CASES):
            raise RuntimeError('missing actual negative source frames')
        return False


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() and any(args.output.iterdir()):
        raise SystemExit('output must be empty')
    if hashlib.sha256(PHOTO.read_bytes()).hexdigest() != PHOTO_SHA256:
        raise SystemExit('original source photo checksum mismatch')
    app = Gtk.Application(application_id='org.rui.NegativeCorpus', flags=Gio.ApplicationFlags.NON_UNIQUE)
    with FallbackProbe() as probe:
        app.connect('activate', lambda app: setattr(app, 'corpus', NegativeCorpus(app, args.output, probe)))
        result = app.run([])
    if result or not (args.output / 'negative-report.json').is_file():
        raise SystemExit('negative source corpus did not finish')


if __name__ == '__main__':
    main()
