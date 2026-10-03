#!/usr/bin/env python3
"""Independent wire decoder for valid fixtures covering all nine scene variants.

This verifies retained scene values and exact wire vocabulary, not pixel output.
Synthetic resource fingerprints do not assert installed fonts or decoded assets.
"""

import json
from pathlib import Path
import random
import struct
import subprocess

try:
    from .reference import fields, one, many, number, double, packed_uints
except ImportError:
    from reference import fields, one, many, number, double, packed_uints

ROOT = Path(__file__).resolve().parent.parent


def nested(message, tag, decoder):
    value = one(message, tag, 2)
    if value is None:
        raise ValueError(f"missing required fixture field {tag}")
    return decoder(fields(value))


def rect(message):
    return {key: double(message, tag) for tag, key in enumerate(("x", "y", "width", "height"), 1)}


def point(message):
    return {"x": double(message, 1), "y": double(message, 2)}


def color(message):
    return {key: double(message, tag) for tag, key in enumerate(("red", "green", "blue", "alpha"), 1)}


def size(message):
    return {"width": double(message, 1), "height": double(message, 2)}


def radii(message):
    return {key: nested(message, tag, size) for tag, key in enumerate(
        ("top_left", "top_right", "bottom_right", "bottom_left"), 1)}


def digest(message, tag):
    value = one(message, tag, 2)
    if value is None or len(value) != 32:
        raise ValueError("invalid fixture digest")
    return list(value)


def stops(message, tag):
    return [{"offset": double(stop, 1), "color": nested(stop, 2, color)}
            for stop in map(fields, many(message, tag, 2))]


def packed_doubles(message, tag):
    values = []
    for wire, raw in message.get(tag, []):
        if wire == 1:
            values.append(struct.unpack("<d", raw)[0])
        elif wire == 2 and len(raw) % 8 == 0:
            values.extend(value[0] for value in struct.iter_unpack("<d", raw))
        else:
            raise ValueError("invalid packed double fixture")
    return values


def primitive(message):
    kinds = [(tag, one(message, tag, 2)) for tag in range(10, 19) if tag in message]
    if len(kinds) != 1:
        raise ValueError("expected exactly one fixture primitive")
    tag, raw = kinds[0]
    item = fields(raw)
    if tag == 10:
        return "Group"
    if tag == 11:
        return {"SolidRect": {"rect": nested(item, 1, rect), "color": nested(item, 2, color)}}
    if tag == 12:
        return {"FillPath": {"resource": digest(item, 1), "color": nested(item, 2, color),
                             "fill_rule": {1: "Nonzero", 2: "EvenOdd"}[number(item, 3)]}}
    if tag == 13:
        font = {"Fingerprint": digest(item, 1)} if 1 in item else {"Outline": digest(item, 2)}
        return {"GlyphRun": {"font": font, "size": double(item, 3),
                             "glyph_ids": packed_uints(item, 4),
                             "positions": [point(fields(raw)) for raw in many(item, 5, 2)],
                             "scalar_clusters": packed_uints(item, 6), "color": nested(item, 7, color)}}
    if tag in (14, 15):
        result = {"rect": nested(item, 1, rect), "radii": nested(item, 2, radii),
                  "color": nested(item, 3 if tag == 14 else 4, color)}
        if tag == 15:
            result["width"] = double(item, 3)
        return {"RoundedRect" if tag == 14 else "Border": result}
    if tag == 16:
        return {"LinearGradient": {"rect": nested(item, 1, rect), "start": nested(item, 2, point),
                                   "end": nested(item, 3, point), "stops": stops(item, 4),
                                   "spread": {1: "Pad"}[number(item, 5)]}}
    if tag == 17:
        return {"RadialGradient": {"rect": nested(item, 1, rect), "center": nested(item, 2, point),
                                   "inner_radius": double(item, 3), "outer_radius": double(item, 4),
                                   "stops": stops(item, 5), "spread": {1: "Pad"}[number(item, 6)]}}
    return {"StrokePath": {"resource": digest(item, 1), "color": nested(item, 2, color),
                           "width": double(item, 3), "cap": {1: "Butt", 2: "Round", 3: "Square"}[number(item, 4)],
                           "join": {1: "Miter", 2: "Round", 3: "Bevel"}[number(item, 5)],
                           "miter_limit": double(item, 6), "dash_lengths": packed_doubles(item, 7),
                           "dash_offset": double(item, 8)}}


def decode_snapshot(payload):
    snapshot = fields(payload)
    records = {}
    for raw in many(snapshot, 2, 2):
        wrapper = fields(raw)
        scene = fields(one(wrapper, 2, 2))
        if scene.get(3):
            raise ValueError("fixture oracle only supports isolated scene roots")
        transform = nested(scene, 4, lambda affine: {
            key: double(affine, tag) for tag, key in enumerate(("xx", "yx", "xy", "yy", "tx", "ty"), 1)})
        records[str(number(scene, 1))] = {"Scene": {
            "id": number(scene, 1), "version": number(scene, 2), "children": [],
            "transform": transform, "clip": nested(scene, 5, rect) if 5 in scene else None,
            "opacity": double(scene, 6), "primitive": primitive(scene),
            "resources": sorted([list(value) for value in set(many(scene, 20, 2))]),
        }}
    return {"revision": number(snapshot, 1), "records": records, "retired": []}


def vint(value):
    result = bytearray()
    while value > 127:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return bytes(result)


def uint(tag, value):
    return vint(tag << 3) + vint(value)


def blob(tag, value):
    return vint((tag << 3) | 2) + vint(len(value)) + value


def floats(*values):
    return b"".join(vint((tag << 3) | 1) + struct.pack("<d", value) for tag, value in enumerate(values, 1))


def scalar(tag, value):
    return vint((tag << 3) | 1) + struct.pack("<d", value)


def fixtures(seed):
    rng = random.Random(seed)
    width, height = rng.randint(32, 128), rng.randint(32, 128)
    rectangle = floats(-2.0, 3.0, width, height)
    corners = b"".join(blob(tag, floats(4.0, 6.0)) for tag in range(1, 5))
    red, blue = floats(0.75, 0.0, 0.0, 0.75), floats(0.0, 0.0, 0.5, 0.75)
    gradient = [floats(0.0) + blob(2, red), floats(0.5) + blob(2, red),
                floats(0.5) + blob(2, blue), floats(1.0) + blob(2, blue)]
    glyph_font_tag = 1 if seed % 2 == 0 else 2
    glyph_font = bytes([2 if glyph_font_tag == 1 else 3]) * 32
    primitives = [
        ("Group", 10, b""),
        ("SolidRect", 11, blob(1, rectangle) + blob(2, red)),
        ("FillPath", 12, blob(1, bytes([1]) * 32) + blob(2, blue) + uint(3, 1 + seed % 2)),
        ("GlyphRun", 13, blob(glyph_font_tag, glyph_font) + scalar(3, 16.0)
         + blob(4, vint(10) + vint(20)) + blob(5, floats(0.0, 20.0))
         + blob(5, floats(12.0, 20.0)) + blob(6, vint(0) + vint(2)) + blob(7, blue)),
        ("RoundedRect", 14, blob(1, rectangle) + blob(2, corners) + blob(3, red)),
        ("Border", 15, blob(1, rectangle) + blob(2, corners) + scalar(3, 2.0) + blob(4, blue)),
        ("LinearGradient", 16, blob(1, rectangle) + blob(2, floats(0.0, 0.0))
         + blob(3, floats(width, height)) + b"".join(blob(4, stop) for stop in gradient) + uint(5, 1)),
        ("RadialGradient", 17, blob(1, rectangle) + blob(2, floats(width / 2, height / 2))
         + scalar(3, 5.0) + scalar(4, 30.0) + b"".join(blob(5, stop) for stop in gradient) + uint(6, 1)),
        ("StrokePath", 18, blob(1, bytes([1]) * 32) + blob(2, red) + scalar(3, 2.0)
         + uint(4, 1 + seed % 3) + uint(5, 1 + seed % 3) + scalar(6, 4.0 if seed % 3 == 0 else 1.0)
         + blob(7, struct.pack("<dd", 4.0, 2.0)) + scalar(8, 1.0)),
    ]
    for name, tag, data in primitives:
        transform = floats(1.0, 0.0, 0.25, 1.0, rng.randint(-100, 100), rng.randint(-100, 100))
        scene = uint(1, 1) + uint(2, 1) + blob(4, transform) + blob(5, rectangle)
        scene += scalar(6, 0.75) + blob(tag, data)
        payload = uint(1, 1) + blob(2, blob(2, scene))
        yield {"name": name, "hex": payload.hex()}


def verify_scenes():
    path = ROOT / "conformance/golden/scenes.jsonl"
    stored = [json.loads(line) for line in path.read_text().splitlines()]
    if stored != list(fixtures(0)):
        raise AssertionError("scene golden bytes changed; review the schema deliberately")
    cases = stored + [case for seed in range(1, 16) for case in fixtures(seed)]
    expected = [{"accepted": True, "state": decode_snapshot(bytes.fromhex(case["hex"]))} for case in cases]
    run = subprocess.run([str(ROOT / "target/debug/examples/scene_replay")],
                         input="".join(json.dumps(case) + "\n" for case in cases),
                         text=True, capture_output=True, cwd=ROOT, check=True)
    actual = [json.loads(line) for line in run.stdout.splitlines()]
    if len(actual) != len(expected):
        raise AssertionError("scene response count differs")
    for index, (left, right) in enumerate(zip(actual, expected)):
        if left != right:
            raise AssertionError(f"scene {index} ({cases[index]['name']}): Rust={left!r}\nPython={right!r}")
    print(f"Independent scene decode passed: {len(cases)} fixtures, all nine primitives")
