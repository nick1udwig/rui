#!/usr/bin/python3
"""Bounded source-native font provider; local producer IPC, never a wire font.

stdin: {"text": Unicode, "size": positive logical px}; stdout: exact RVP1
resources + source-shaped glyph0 runs. Failures reveal no input text. The
source broker must validate visual output before publication. Valid semantic
edits survive shaping failure with explicit Partial/Unsupported visual coverage.
"""
import ctypes
import ctypes.util
import hashlib
import json
import math
from pathlib import Path
import sys

ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT))
FONT=Path(__file__).with_name("fonts")/"LiberationSans-Regular.ttf"
FONT_SHA="baccc64becc3eb7d104b7c84d99f5314a0a1f896e2b3ea6c2f22fc08d2003bee"
PROFILE="source.pango1582.cairo1184.liberation-sans.v1"

def configure_font():
    if FONT.stat().st_size!=410820 or hashlib.sha256(FONT.read_bytes()).hexdigest()!=FONT_SHA:
        raise ValueError("pinned font differs")
    # A process-private empty Fontconfig config admits only this exact source
    # font. No system fallback can silently substitute another font or emoji.
    fc=ctypes.CDLL(ctypes.util.find_library("fontconfig"))
    fc.FcConfigCreate.restype=ctypes.c_void_p
    fc.FcConfigAppFontAddFile.argtypes=[ctypes.c_void_p,ctypes.c_char_p]
    fc.FcConfigAppFontAddFile.restype=ctypes.c_int
    fc.FcConfigSetCurrent.argtypes=[ctypes.c_void_p]
    fc.FcConfigSetCurrent.restype=ctypes.c_int
    config=fc.FcConfigCreate()
    if not config or not fc.FcConfigAppFontAddFile(config,str(FONT).encode()) or not fc.FcConfigSetCurrent(config):
        raise ValueError("source font setup failed")

def shape(text,size):
    import cairo
    import gi
    for name,version in (("Gtk","4.0"),("Gsk","4.0"),("Pango","1.0"),("PangoCairo","1.0")):
        gi.require_version(name,version)
    from gi.repository import Gtk,Gdk,Pango,PangoCairo
    from adapters.gtk4.capture import Exporter
    from adapters.sdk.producer import CoherentSource
    if (Gtk.get_major_version(),Gtk.get_minor_version(),Gtk.get_micro_version())!=(4,22,4) or Pango.version_string()!="1.58.2" or cairo.cairo_version_string()!="1.18.4":
        raise ValueError("pinned source libraries differ")
    configure_font()
    context=cairo.Context(cairo.ImageSurface(cairo.FORMAT_A8,1,1))
    layout=PangoCairo.create_layout(context)
    description=Pango.FontDescription.from_string("Liberation Sans")
    description.set_absolute_size(size*Pango.SCALE)
    layout.set_font_description(description)
    layout.set_text(text,-1)
    snapshot=Gtk.Snapshot.new()
    snapshot.append_layout(layout,Gdk.RGBA(0,0,0,1))
    node=snapshot.to_node()
    source=CoherentSource();exporter=Exporter(source,"font",1)
    if node is not None: exporter.export(node)
    if exporter.gaps or len(exporter.glyph_evidence)>1024:
        raise ValueError("source font cannot faithfully represent this input")
    runs=[]
    for record in exporter.scene:
        primitive=record["primitive"]
        if primitive["type"]=="GlyphRun":
            if record["transform"]!=[1,0,0,1,0,0] or record["clip"] is not None:
                raise ValueError("unsupported source text transform")
            runs.append({"resource":primitive["resource"],"size":primitive["size"],"position":primitive["positions"][0]})
    ink,logical=layout.get_pixel_extents()
    resources=[]
    total=0
    for digest,resource in sorted(source.resources.items()):
        total+=resource["total_bytes"]
        if total>4<<20 or int.from_bytes(resource["body"][4:8],"big")>4096:
            raise ValueError("source outline ceiling")
        resources.append({key:value for key,value in resource.items() if key!="body"}|{"body_hex":resource["body"].hex()})
    return {"profile":PROFILE,"font_sha256":FONT_SHA,"logical_bounds":[logical.x,logical.y,logical.width,logical.height],"ink_bounds":[ink.x,ink.y,ink.width,ink.height],"baseline":layout.get_baseline()/Pango.SCALE,"glyphs":runs,"resources":resources,"source_glyph_evidence":exporter.glyph_evidence}

def main():
    raw=sys.stdin.buffer.read(4097)
    if len(raw)>4096:raise ValueError("source request ceiling")
    request=json.loads(raw)
    text=request["text"];size=float(request["size"])
    if not isinstance(text,str) or len(text.encode())>2048 or not math.isfinite(size) or not 1<=size<=256:
        raise ValueError("source request outside profile")
    result=json.dumps(shape(text,size),separators=(",",":"),sort_keys=True).encode()
    if len(result)>8<<20:raise ValueError("source output ceiling")
    sys.stdout.buffer.write(result+b"\n")

if __name__=="__main__":
    try:main()
    except Exception:
        print("source font provider rejected input or environment",file=sys.stderr)
        sys.exit(1)
