"""Test-only Python API traps during source lowering, never a wire feature.

Native GTK may rasterize locally. These traps cover exporter Python calls to
pixel download/save, node serialization/draw and image-backed Cairo drawing;
native GTK internals are outside this observation boundary.
"""
from contextlib import ExitStack
from collections import Counter
from unittest.mock import patch

import cairo
import gi
for name, version in [('Gdk', '4.0'), ('Gsk', '4.0')]:
    gi.require_version(name, version)
from gi.repository import Gdk, Gsk
from adapters.gtk4.capture import Exporter


class ForbiddenFallback(RuntimeError):
    pass


class FallbackProbe:
    def __init__(self):
        self.active = 0
        self.attempts = Counter()
        self.outside = Counter()
        self.controls = Counter()
        self.stack = ExitStack()

    def __enter__(self):
        original_export = Exporter.export
        def export(exporter, *args, **kwargs):
            self.active += 1
            try:
                return original_export(exporter, *args, **kwargs)
            finally:
                self.active -= 1
        self.stack.enter_context(patch.object(Exporter, 'export', export))
        for owner, methods in [(Gdk.Texture, ['download', 'save_to_png', 'save_to_png_bytes', 'save_to_tiff', 'save_to_tiff_bytes']),
                               (Gsk.RenderNode, ['draw', 'serialize', 'write_to_file'])]:
            for method in methods:
                key = owner.__name__ + '.' + method
                original = getattr(owner, method)
                def trap(*args, _key=key, _original=original, **kwargs):
                    if self.active:
                        self.attempts[_key] += 1
                        raise ForbiddenFallback(_key)
                    self.outside[_key] += 1
                    return _original(*args, **kwargs)
                self.stack.enter_context(patch.object(owner, method, trap))
        original_surface = cairo.ImageSurface
        def surface(format_, width, height, *args, **kwargs):
            # The existing native glyph-path extractor needs a one-pixel A8
            # Cairo context but never draws to it. Larger/color surfaces during
            # export would permit an image-backed conversion and are trapped.
            if self.active and (format_, width, height) != (cairo.FORMAT_A8, 1, 1):
                self.attempts['cairo.ImageSurface'] += 1
                raise ForbiddenFallback('cairo.ImageSurface')
            return original_surface(format_, width, height, *args, **kwargs)
        self.stack.enter_context(patch.object(cairo, 'ImageSurface', surface))
        return self

    def prove_traps(self, texture):
        """Intentional negative controls, accounted separately from exporting."""
        before = self.attempts.copy()
        self.active += 1
        try:
            for name, call in [('Texture.download', lambda: texture.download(bytearray(16), 8)),
                               ('ImageSurface', lambda: cairo.ImageSurface(cairo.FORMAT_ARGB32, 4, 4))]:
                try:
                    call()
                except ForbiddenFallback:
                    self.controls[name] += 1
                else:
                    raise AssertionError('forbidden-path control did not trip: ' + name)
        finally:
            self.active -= 1
            self.attempts = before

    def report(self):
        return {'scope': 'exporter-Python-API-only-native-local-rendering-excluded',
                'forbidden_export_calls': dict(sorted(self.attempts.items())),
                'allowed_outside_export_calls': dict(sorted(self.outside.items())),
                'negative_controls_tripped': dict(sorted(self.controls.items()))}

    def __exit__(self, *args):
        return self.stack.__exit__(*args)
