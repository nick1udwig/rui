"""Pinned GTK4.22.4 cooperating snapshot producer and conservative exporter."""
from __future__ import annotations

from collections import Counter, deque
import ctypes
import ctypes.util
import hashlib
import math
import re
import time

import cairo
import gi

for _name, _version in [("Gtk", "4.0"), ("Gsk", "4.0"), ("Graphene", "1.0"), ("PangoCairo", "1.0")]:
    gi.require_version(_name, _version)
from gi.repository import Gtk, Gsk, Pango, PangoCairo

from adapters.sdk.producer import CoherentSource, path_resource


PINNED_GTK = (4, 22, 4)
MAX_NODES = 4096
MAX_GLYPHS = 4096


class Unsupported(Exception):
    pass


class GlyphInfo(ctypes.Structure):
    _fields_ = [("glyph", ctypes.c_uint), ("width", ctypes.c_int32), ("x_offset", ctypes.c_int32), ("y_offset", ctypes.c_int32), ("attributes", ctypes.c_uint)]


_gtk = ctypes.CDLL(ctypes.util.find_library("gtk-4"))
_gtk.gsk_render_node_get_node_type.argtypes = [ctypes.c_void_p]
_gtk.gsk_render_node_get_node_type.restype = ctypes.c_int
_gtk.gsk_text_node_get_glyphs.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint)]
_gtk.gsk_text_node_get_glyphs.restype = ctypes.POINTER(GlyphInfo)


def glyph_infos(node):
    """Read the pinned PangoGlyphInfo ABI, bypassing a broken GI array stride.

    PyGObject's boxed RenderNode hash is its native address on this pinned build.
    Verify node type through C before dereferencing; never accept peer pointers.
    """
    pointer = hash(node)
    if _gtk.gsk_render_node_get_node_type(pointer) != int(node.get_node_type()):
        raise Unsupported("pinned-gsk-boxed-pointer-abi")
    count = ctypes.c_uint()
    values = _gtk.gsk_text_node_get_glyphs(pointer, ctypes.byref(count))
    if count.value != node.get_num_glyphs() or count.value > MAX_GLYPHS:
        raise Unsupported("glyph-count-ceiling")
    return [(values[i].glyph, values[i].width, values[i].x_offset, values[i].y_offset, values[i].attributes) for i in range(count.value)]


def rect(value):
    return [value.origin.x, value.origin.y, value.size.width, value.size.height]


def color(value):
    # Gdk.RGBA is unpremultiplied sRGB; RUI uses premultiplied linear sRGB.
    # Alpha composition in GTK's default sRGB pipeline is not equivalent, so
    # this producer only exports opaque flat paint; effects remain gaps.
    if value.alpha != 1:
        raise Unsupported("nonopaque-srgb-composition")
    def linear(channel):
        return channel / 12.92 if channel <= .04045 else ((channel + .055) / 1.055) ** 2.4
    return [linear(value.red), linear(value.green), linear(value.blue), 1]


def radius(value):
    return [[corner.width, corner.height] for corner in value.corner]


def check_radii(pairs, width, height):
    if any(x < 0 or y < 0 or ((x == 0) != (y == 0)) for x, y in pairs):
        raise Unsupported("corner-radii-outside-rui-profile")
    if pairs[0][0] + pairs[1][0] > width or pairs[3][0] + pairs[2][0] > width or pairs[0][1] + pairs[3][1] > height or pairs[1][1] + pairs[2][1] > height:
        raise Unsupported("corner-edge-sums-outside-rui-profile")


def check_border(outline, width):
    if width < 0 or width > min(outline.bounds.size.width, outline.bounds.size.height) / 2:
        raise Unsupported("border-width-not-inward-profile")
    outer = radius(outline)
    check_radii(outer, outline.bounds.size.width, outline.bounds.size.height)
    inner = [[max(x - width, 0), max(y - width, 0)] for x, y in outer]
    inner = [[0, 0] if x == 0 or y == 0 else [x, y] for x, y in inner]
    check_radii(inner, outline.bounds.size.width - 2 * width, outline.bounds.size.height - 2 * width)


def cairo_commands(path, scale=1):
    commands = []
    for opcode, values in path:
        mapped = {cairo.PATH_MOVE_TO: 1, cairo.PATH_LINE_TO: 2, cairo.PATH_CURVE_TO: 4, cairo.PATH_CLOSE_PATH: 5}[opcode]
        commands.append((mapped, tuple(number / scale for number in values)))
    return commands


def gsk_commands(path):
    # GSK writes its exact float geometry in a round-trippable path string.
    # Conic/arc forms need equivalence-preserving conversion and are rejected.
    encoded = path.to_string()
    tokens = re.findall(r"[A-Za-z]|[-+]?(?:\d*\.\d+|\d+\.?\d*)(?:[eE][-+]?\d+)?", encoded)
    counts = {"M": 2, "L": 2, "Q": 4, "C": 6, "Z": 0, "z": 0}
    opcodes = {"M": 1, "L": 2, "Q": 3, "C": 4, "Z": 5, "z": 5}
    commands = []
    index = 0
    while index < len(tokens):
        opcode = tokens[index]
        if opcode not in counts:
            raise Unsupported("unsupported-gsk-path-operation")
        count = counts[opcode]
        coordinates = tuple(float(x) for x in tokens[index + 1:index + count + 1])
        if len(coordinates) != count:
            raise Unsupported("malformed-gsk-path")
        commands.append((opcodes[opcode], coordinates))
        index += count + 1
    return commands


class Exporter:
    def __init__(self, source, surface, revision):
        self.source = source
        self.surface = surface
        self.revision = revision
        self.scene = []
        self.gaps = []
        self.resources = set()
        self.uses_glyph_outline = False
        self.glyph_evidence = []
        self.visited = 0
        self.depth = 0

    def record(self, key, primitive, children=(), transform=None, clip=None):
        if len(self.scene) >= 20_000:
            raise Unsupported("scene-record-ceiling")
        identifier = self.source.identifier("scene:" + self.surface + ":" + key)
        self.scene.append({"id": identifier, "version": self.revision, "children": list(children), "primitive": primitive, "transform": transform or [1, 0, 0, 1, 0, 0], "clip": clip, "opacity": 1})
        return identifier

    def resource(self, commands, glyph=False):
        resource = path_resource(commands, glyph)
        total = sum(len(item["body"]) for item in self.source.resources.values())
        if resource["digest"] not in self.source.resources and total + len(resource["body"]) > 32 << 20:
            raise Unsupported("source-resource-ceiling")
        self.source.resources[resource["digest"]] = resource
        self.resources.add(resource["digest"])
        return resource["digest"]

    def export(self, node, path="root"):
        self.visited += 1
        if self.visited > MAX_NODES:
            raise Unsupported("source-node-ceiling")
        previous_count = len(self.scene)
        previous_resources = set(self.resources)
        previous_glyphs = self.uses_glyph_outline
        previous_evidence = len(self.glyph_evidence)
        previous_depth = self.depth
        try:
            # Leave one scene level for a text node's per-glyph children and
            # the bounded placeholder, so the default reducer depth128 holds.
            if self.depth >= 127:
                raise Unsupported("source-depth-ceiling")
            self.depth += 1
            return self.supported(node, path)
        except Unsupported as error:
            del self.scene[previous_count:]
            self.resources = previous_resources
            self.uses_glyph_outline = previous_glyphs
            del self.glyph_evidence[previous_evidence:]
            self.gaps.append({"path": path, "reason": str(error), "node_type": node.get_node_type().value_nick, "bounds": rect(node.get_bounds())})
            return self.record(path, {"type": "Group"})
        finally:
            self.depth = previous_depth

    def supported(self, node, path):
        if isinstance(node, Gsk.ContainerNode):
            return self.record(path, {"type": "Group"}, [self.export(child, path + "/" + str(i)) for i, child in enumerate(node.get_children() or [])])
        if isinstance(node, Gsk.TransformNode):
            if int(node.get_transform().get_category()) < int(Gsk.TransformCategory.__enum_values__[3]):
                raise Unsupported("non-2d-transform")
            return self.record(path, {"type": "Group"}, [self.export(node.get_child(), path + "/child")], list(node.get_transform().to_2d()))
        if isinstance(node, Gsk.ColorNode):
            return self.record(path, {"type": "SolidRect", "rect": rect(node.get_bounds()), "color": color(node.get_color())})
        if isinstance(node, Gsk.ClipNode):
            return self.record(path, {"type": "Group"}, [self.export(node.get_child(), path + "/child")], clip=rect(node.get_clip()))
        if isinstance(node, Gsk.RoundedClipNode):
            clip = node.get_clip()
            if any(any(value != 0 for value in pair) for pair in radius(clip)):
                child = node.get_child()
                if isinstance(child, Gsk.ColorNode) and rect(child.get_bounds()) == rect(clip.bounds):
                    check_radii(radius(clip), clip.bounds.size.width, clip.bounds.size.height)
                    return self.record(path, {"type": "RoundedRect", "rect": rect(clip.bounds), "radii": radius(clip), "color": color(child.get_color())})
                raise Unsupported("rounded-clip-on-nonuniform-content")
            return self.record(path, {"type": "Group"}, [self.export(node.get_child(), path + "/child")], clip=rect(clip.bounds))
        if isinstance(node, Gsk.BorderNode):
            widths, colors = node.get_widths(), node.get_colors()
            if len(set(widths)) != 1 or len({(v.red, v.green, v.blue, v.alpha) for v in colors}) != 1:
                raise Unsupported("nonuniform-border")
            outline = node.get_outline()
            check_border(outline, widths[0])
            return self.record(path, {"type": "Border", "rect": rect(outline.bounds), "radii": radius(outline), "width": widths[0], "color": color(colors[0])})
        if isinstance(node, Gsk.TextNode):
            if node.has_color_glyphs():
                raise Unsupported("color-glyphs")
            paint = color(node.get_color())
            scaled = PangoCairo.Font.get_scaled_font(node.get_font())
            if scaled is None:
                raise Unsupported("non-cairo-font-backend")
            matrix = scaled.get_font_matrix()
            if matrix.xx != matrix.yy or matrix.xy != 0 or matrix.yx != 0 or matrix.xx <= 0:
                raise Unsupported("nonuniform-font-matrix")
            size = matrix.yy
            offset = node.get_offset()
            advance = 0.0
            children = []
            context = cairo.Context(cairo.ImageSurface(cairo.FORMAT_A8, 1, 1))
            context.set_scaled_font(scaled)
            for index, (glyph, width, x_offset, y_offset, attributes) in enumerate(glyph_infos(node)):
                if glyph & (1 << 28) or attributes & 2:
                    raise Unsupported("missing-or-color-glyph")
                if glyph != 0x0FFFFFFF:
                    context.new_path()
                    context.glyph_path([(glyph, 0, 0)])
                    commands = cairo_commands(context.copy_path(), size)
                    if commands:
                        digest = self.resource(commands, glyph=True)
                        position = [offset.x + advance + x_offset / Pango.SCALE, offset.y + y_offset / Pango.SCALE]
                        identifier = self.record(path + "/glyph/" + str(index), {"type": "GlyphRun", "resource": digest, "size": size, "glyph_ids": [0], "positions": [position], "color": paint})
                        children.append(identifier)
                        self.glyph_evidence.append({"scene_id": identifier, "source_glyph_id": glyph, "source_font": node.get_font().describe().to_string(), "font_size": size, "node_offset": [offset.x, offset.y], "advance": advance, "pango_width": width, "pango_x_offset": x_offset, "pango_y_offset": y_offset, "pango_scale": Pango.SCALE, "path_bounds_em": [value / size for value in context.path_extents()], "resource": digest})
                        self.uses_glyph_outline = True
                advance += width / Pango.SCALE
            return self.record(path, {"type": "Group"}, children)
        if isinstance(node, Gsk.FillNode):
            child = node.get_child()
            if not isinstance(child, Gsk.ColorNode):
                raise Unsupported("nonuniform-path-paint")
            digest = self.resource(gsk_commands(node.get_path()))
            return self.record(path, {"type": "FillPath", "resource": digest, "fill_rule": 1 if int(node.get_fill_rule()) == 0 else 2, "color": color(child.get_color())})
        if isinstance(node, Gsk.StrokeNode):
            child = node.get_child()
            if not isinstance(child, Gsk.ColorNode):
                raise Unsupported("nonuniform-stroke-paint")
            stroke = node.get_stroke()
            cap = {0: 1, 1: 2, 2: 3}.get(int(stroke.get_line_cap()))
            join = {0: 1, 1: 2, 2: 3}.get(int(stroke.get_line_join()))
            dashes = list(stroke.get_dash() or [])
            if cap is None or join is None or len(dashes) % 2 or any(value <= 0 for value in dashes):
                raise Unsupported("unsupported-stroke-style")
            period = sum(dashes)
            dash_offset = stroke.get_dash_offset()
            if dash_offset < 0 or (period and dash_offset >= period) or (not period and dash_offset != 0):
                raise Unsupported("unnormalized-stroke-dash-offset")
            digest = self.resource(gsk_commands(node.get_path()))
            return self.record(path, {"type": "StrokePath", "resource": digest, "color": color(child.get_color()), "width": stroke.get_line_width(), "cap": cap, "join": join, "miter_limit": stroke.get_miter_limit() if join == 1 else 1, "dash_lengths": dashes, "dash_offset": dash_offset})
        # In particular, sRGB gradients, shadows, Cairo/texture content, masks,
        # blends and nontrivial opacity never become hidden raster fallbacks.
        raise Unsupported("unsupported-" + node.get_node_type().value_nick)


def census(node):
    counts = Counter()
    bounds = []
    def visit(current, depth=0):
        name = current.get_node_type().value_nick
        counts[name] += 1
        bounds.append({"type": name, "bounds": rect(current.get_bounds())})
        if depth == 127:
            bounds[-1]["truncated"] = "source-depth-ceiling"
            return
        for child in current.get_children() or []:
            if sum(counts.values()) >= MAX_NODES:
                bounds[-1]["truncated"] = "source-node-ceiling"
                break
            visit(child, depth + 1)
    visit(node)
    return dict(sorted(counts.items())), bounds


class CaptureMixin:
    """Capture exactly the content appended during the actual snapshot vfunc.

    This covers this cooperating widget's subtree, not window decorations,
    compositor effects or separately registered popup surfaces.
    """
    def setup_capture(self, source: CoherentSource, surface_key, semantic_root, output, on_capture=None, role=1):
        actual = (Gtk.get_major_version(), Gtk.get_minor_version(), Gtk.get_micro_version())
        if actual != PINNED_GTK:
            raise RuntimeError(f"GTK pin mismatch: {actual}, expected {PINNED_GTK}")
        self.source = source
        self.surface_key = surface_key
        self.semantic_root = semantic_root
        self.output = output
        self.on_capture = on_capture
        self.role = role
        self.frames = 0
        self.captures = deque(maxlen=64)
        self.focus_reader = None
        self.source_alive = True
        self.source_mapped = False
        self.last_frame = None

    def source_boundary(self):
        if not self.source_alive:
            raise RuntimeError("retired source surface cannot capture")
        if self.focus_reader:
            self.source.focus_changed(self.surface_key, self.focus_reader())
        return self.source.source_revision

    def capture_node(self, submitted_snapshot, node, source_revision, started):
        if node is None:
            return
        # Critical boundary: the very same immutable GskRenderNode object
        # becomes the actual local snapshot child. No WidgetPaintable sampling.
        submitted_snapshot.append_node(node)
        self.source_mapped = True
        self.frames += 1
        semantics = self.source.semantics(self.semantic_root, self.frames)
        if source_revision != self.source.source_revision:
            raise RuntimeError("source mutated during coherent capture")
        token = f"{self.source.identity}:{self.surface_key}:{source_revision}:{self.frames}"
        exporter = Exporter(self.source, self.surface_key, self.frames)
        scene_root = exporter.export(node)
        self.source.reconcile_scene(self.surface_key, exporter.scene)
        counts, region_bounds = census(node)
        frame = {"revision": self.frames, "source_revision": source_revision, "surface_key": self.surface_key, "surface_id": self.source.identifier("surface:" + self.surface_key), "surface_role": self.role, "size": [self.get_width(), self.get_height()], "capture_token": token, "capture_boundary": "actual-snapshot-vfunc-same-node-append", "scene_root": scene_root, "semantic_root": self.source.identifier("semantic:" + self.semantic_root), "semantics": semantics, "scene": exporter.scene, "gaps": exporter.gaps, "resources": sorted(exporter.resources), "uses_glyph_outline": exporter.uses_glyph_outline, "gsk_node_counts": counts, "gsk_node_bounds": region_bounds, "capture_duration_ns": time.monotonic_ns() - started, "glyph_source": "native-PangoGlyphInfo+Cairo-source-scaled-font", "gtk_version": list(PINNED_GTK)}
        frame["source_glyph_evidence"] = exporter.glyph_evidence
        frame["session"] = self.source.session(self.surface_key, self.frames, frame["size"], {record["id"] for record in semantics})
        frame["focused"] = frame["session"]["focus"] is not None
        frame["mapped"] = True
        if getattr(self, "parent_capture", None) is not None:
            parent = self.parent_capture
            frame["parent_surface"] = {"surface_id": self.source.identifier("surface:" + parent.surface_key), "size": [parent.get_width(), parent.get_height()]}
            surface = self.get_native().get_surface()
            if not hasattr(surface, "get_position_x"):
                raise RuntimeError("popup backend cannot supply explicit parent-relative geometry")
            frame["origin"] = [surface.get_position_x(), surface.get_position_y()]
        # Local test diagnostic only: these image bytes never enter scene,
        # envelope, resource offers or provenance. The exact submitted node is
        # independently rasterized by the pinned native Cairo/GSK implementation.
        width, height = self.get_width(), self.get_height()
        if width <= 0 or height <= 0 or width * height > 1_048_576:
            raise RuntimeError("local diagnostic pixel ceiling")
        reference = cairo.ImageSurface(cairo.FORMAT_ARGB32, width, height)
        node.draw(cairo.Context(reference))
        reference.flush()
        raw = bytes(reference.get_data())
        frame["local_reference"] = {"width": width, "height": height, "stride": reference.get_stride(), "format": "cairo-ARGB32-native-little-endian-BGRA-premultiplied-sRGB", "sha256": hashlib.sha256(raw).hexdigest(), "scope": "local-comparison-only-never-wire"}
        label = self.source.write(self.output, frame)
        reference.write_to_png(str(self.output / (label + ".local.png")))
        (self.output / (label + ".local.bgra")).write_bytes(raw)
        self.captures.append({"label": label, "source_revision": source_revision, "counts": counts, "gaps": frame["gaps"]})
        # Publish bookkeeping before an observer can synchronously unmap or
        # destroy the native source. Such a lifecycle publication owns the new
        # last_frame; returning from this callback must not overwrite it.
        self.last_frame = frame
        if self.on_capture:
            self.on_capture(self, frame)

    def publish_lifecycle(self, destroyed=False):
        """Source-issued lifecycle evidence; no synthetic visual frame claim."""
        if not self.source_alive or self.last_frame is None or (not self.source_mapped and not destroyed):
            return
        self.source.changed()
        self.source.focus_changed(self.surface_key, None)
        self.source_mapped = False
        self.frames += 1
        exporter = Exporter(self.source, self.surface_key, self.frames)
        root = None if destroyed else exporter.record("unmapped", {"type": "Group"})
        self.source.reconcile_scene(self.surface_key, exporter.scene)
        semantics = [] if destroyed else self.source.semantics(self.semantic_root, self.frames)
        frame = {**self.last_frame, "revision": self.frames, "source_revision": self.source.source_revision,
                 "capture_boundary": "source-lifecycle-signal", "capture_token": f"{self.source.identity}:{self.surface_key}:{self.source.source_revision}:{self.frames}",
                 "mapped": False, "focused": False, "destroyed": destroyed, "scene_root": root,
                 "semantic_root": None if destroyed else self.source.identifier("semantic:" + self.semantic_root),
                 "scene": exporter.scene, "semantics": semantics, "resources": [], "gaps": [],
                 "source_glyph_evidence": [], "uses_glyph_outline": False,
                 "gsk_node_counts": {}, "gsk_node_bounds": [], "capture_duration_ns": 0}
        frame.pop("local_reference", None)
        frame["session"] = self.source.session(self.surface_key, self.frames, frame["size"], set(), mapped=False)
        self.source.write(self.output, frame)
        self.last_frame = frame
        if destroyed:
            self.source_alive = False
            self.source.unregister(self.semantic_root)
            self.source.retire_surface(self.surface_key)
        if self.on_capture:
            self.on_capture(self, frame)

    def retire_source(self):
        if self.source_alive and self.last_frame is None:
            # No mapped publication exists to remove, but disposal still burns
            # any registered source identities and forbids later capture.
            self.source_alive = False
            self.source.unregister(self.semantic_root)
            self.source.retire_surface(self.surface_key)
            return
        self.publish_lifecycle(destroyed=True)


class CaptureWidget(CaptureMixin, Gtk.Widget):
    def __init__(self, source, child, surface_key, semantic_root, output, on_capture=None, role=1):
        super().__init__()
        self.set_layout_manager(Gtk.BinLayout())
        child.set_parent(self)
        self.child = child
        self.setup_capture(source, surface_key, semantic_root, output, on_capture, role)

    def do_snapshot(self, submitted_snapshot):
        started = time.monotonic_ns()
        revision = self.source_boundary()
        captured = Gtk.Snapshot()
        self.snapshot_child(self.child, captured)
        self.capture_node(submitted_snapshot, captured.to_node(), revision, started)


class CapturePopover(CaptureMixin, Gtk.PopoverMenu):
    """Preserve native menu internals and capture its real parent snapshot."""
    def __init__(self, source, menu_model, surface_key, semantic_root, output, on_capture=None):
        super().__init__(menu_model=menu_model)
        self.setup_capture(source, surface_key, semantic_root, output, on_capture, role=2)
        self.connect('unmap', lambda *_: self.publish_lifecycle())

    def do_snapshot(self, submitted_snapshot):
        started = time.monotonic_ns()
        revision = self.source_boundary()
        captured = Gtk.Snapshot()
        Gtk.PopoverMenu.do_snapshot(self, captured)
        self.capture_node(submitted_snapshot, captured.to_node(), revision, started)
