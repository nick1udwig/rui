#!/usr/bin/env python3
"""Offline compression probes; these formats are not deployed RUI protocols."""

import argparse
from compression import zstd
import gzip
import hashlib
import json
import math
from pathlib import Path
import platform
import struct
import subprocess
import tempfile
import time
import zlib

ROOT = Path(__file__).resolve().parents[3]
DEFAULT_EVIDENCE = ROOT / "demos/games/evidence/comparison"
WINDOW_LOG = 18
SCALES = (10000,) * 4 + (1000000,) * 3


def sha(data):
    return hashlib.sha256(data).hexdigest()


def publications(data):
    result = []
    offset = 0
    while offset < len(data):
        if offset + 4 > len(data):
            raise ValueError("truncated publication length")
        size = struct.unpack_from(">I", data, offset)[0]
        offset += 4
        if size == 0 or offset + size > len(data):
            raise ValueError("invalid publication size")
        result.append(data[offset:offset + size])
        offset += size
    if not result:
        raise ValueError("empty publication stream")
    return result


def percentile(values, fraction):
    return sorted(values)[math.ceil(len(values) * fraction) - 1]


def compress_frames(frames, codec, level, reset, repeats=3, window_log=WINDOW_LOG):
    """Flush each publication and prove it decodes before seeing the next one."""
    reset = reset or len(frames)
    sizes = None
    encode_ms = []
    decode_ms = []
    for _ in range(repeats):
        current_sizes = []
        for i, frame in enumerate(frames):
            first = i % reset == 0
            last = (i + 1) % reset == 0 or i + 1 == len(frames)
            started = time.perf_counter_ns()
            if first:
                if codec == "zstd":
                    compressor = zstd.ZstdCompressor(options={
                        zstd.CompressionParameter.compression_level: level,
                        zstd.CompressionParameter.window_log: window_log,
                    })
                else:
                    compressor = zlib.compressobj(level)
            if codec == "zstd":
                encoded = compressor.compress(frame, mode=compressor.FLUSH_FRAME if last else compressor.FLUSH_BLOCK)
            else:
                encoded = compressor.compress(frame) + compressor.flush(zlib.Z_FINISH if last else zlib.Z_SYNC_FLUSH)
            encode_ms.append((time.perf_counter_ns() - started) / 1e6)
            started = time.perf_counter_ns()
            if first:
                if codec == "zstd":
                    decoder = zstd.ZstdDecompressor(options={zstd.DecompressionParameter.window_log_max: window_log})
                else:
                    decoder = zlib.decompressobj()
            decoded = decoder.decompress(encoded)
            decode_ms.append((time.perf_counter_ns() - started) / 1e6)
            if decoded != frame or decoder.eof != last or decoder.unused_data:
                raise ValueError("incremental compression round trip failed")
            current_sizes.append(len(encoded))
        if sizes is not None and sizes != current_sizes:
            raise ValueError("compression sizes changed between repetitions")
        sizes = current_sizes
    return {
        "bytes": sum(sizes),
        "frame_bytes": sizes,
        "first_frame_bytes": sizes[0],
        "codec": codec,
        "level": level,
        "history_bytes": 2 ** window_log if codec == "zstd" else 32768,
        "reset_frames": reset,
        "repetitions": repeats,
        "encode_call_ms_p95": percentile(encode_ms, 0.95),
        "decode_call_ms_p95": percentile(decode_ms, 0.95),
        "exact_incremental_round_trip": True,
    }


def float_bytes(lines):
    return b"".join(struct.pack("<7d", *line) for line in lines)


def varint(value):
    result = bytearray()
    while value > 127:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return result


def read_varints(data):
    result = []
    value = shift = 0
    for byte in data:
        value |= (byte & 127) << shift
        if byte < 128:
            result.append(value)
            value = shift = 0
        else:
            shift += 7
            if shift > 63:
                raise ValueError("oversize varint")
    if shift:
        raise ValueError("truncated varint")
    return result


def beam_arrays(captures, style, reset):
    """Pack source geometry and reconstruct every IEEE float without a loss."""
    encoded = []
    decoded = []
    prior_encode = []
    prior_decode = []
    for i, capture in enumerate(captures):
        lines = capture["lines"]
        if i % reset == 0:
            prior_encode = []
            prior_decode = []
        if style == "decimal-delta":
            integers = [round(value * SCALES[j]) for line in lines for j, value in enumerate(line)]
            body = bytearray()
            for j, value in enumerate(integers):
                delta = value - (prior_encode[j] if j < len(prior_encode) else 0)
                body.extend(varint(2 * delta if delta >= 0 else -2 * delta - 1))
            prior_encode = integers
        else:
            body = float_bytes(lines)
            if style == "byte-shuffle":
                body = b"".join(body[j::8] for j in range(8))
        packet = struct.pack("<I", len(lines)) + body
        count = struct.unpack_from("<I", packet)[0]
        body = packet[4:]
        if style == "decimal-delta":
            deltas = read_varints(body)
            if len(deltas) != count * 7:
                raise ValueError("wrong decimal field count")
            integers = [(value // 2 if value % 2 == 0 else -(value // 2) - 1) + (prior_decode[j] if j < len(prior_decode) else 0) for j, value in enumerate(deltas)]
            prior_decode = integers
            values = [value / SCALES[j % 7] for j, value in enumerate(integers)]
            rebuilt = [values[j:j + 7] for j in range(0, len(values), 7)]
        else:
            if len(body) != count * 56:
                raise ValueError("wrong float field count")
            if style == "byte-shuffle":
                words = count * 7
                body = bytes(body[byte * words + word] for word in range(words) for byte in range(8))
            rebuilt = [list(line) for line in struct.iter_unpack("<7d", body)]
        if float_bytes(rebuilt) != float_bytes(lines):
            raise ValueError("source values are not exactly representable by the selected beam format")
        encoded.append(packet)
        decoded.append({**capture, "lines": rebuilt})
    return encoded, decoded


def verify_projector(binary, captures, expected):
    with tempfile.TemporaryDirectory(prefix="rui-strategy-") as directory:
        root = Path(directory)
        source = root / "frames.jsonl"
        target = root / "publications.rui"
        source.write_text("".join(json.dumps(frame) + "\n" for frame in captures))
        subprocess.run([str(binary), str(source), str(target), str(root / "publications.json")], check=True, capture_output=True)
        if target.read_bytes() != expected:
            raise ValueError("decoded source does not rebuild the original RUI publications")
    return True


def rate(result, seconds):
    result["mbps"] = result["bytes"] * 8 / seconds / 1e6
    return result


def analyze(evidence, repeats, binary):
    reference = json.loads((evidence / "summary.json").read_text())
    result = {
        "scope": "offline compression of retained publications and exact beam sources; not a deployed transport",
        "python": platform.python_version(),
        "zstd": zstd.zstd_version,
        "zlib": zlib.ZLIB_RUNTIME_VERSION,
        "script_sha256": sha(Path(__file__).read_bytes()),
        "projector_sha256": sha((ROOT / "browser-host/src/projection.rs").read_bytes()),
        "exporter_sha256": sha((ROOT / "browser-host/src/bin/game-trace.rs").read_bytes()),
        "timing_scope": "Python compression and decompression calls, including context setup; all frames, three runs by default; excludes beam packing, rendering, network, browser, and memory profiling",
        "games": [],
    }
    for reference_game in reference["games"]:
        name = reference_game["capture"]["game"]
        directory = evidence / name
        source = gzip.decompress((directory / "frames.jsonl.gz").read_bytes())
        capture = json.loads((directory / "capture.json").read_text())
        if sha(source) != capture["trace_sha256"]:
            raise ValueError("source trace hash mismatch")
        source_frames = [json.loads(line) for line in source.splitlines()]
        raw = gzip.decompress((directory / "publications.rui.gz").read_bytes())
        frames = publications(raw)
        if len(frames) != len(source_frames) or len(frames) != capture["frames"]:
            raise ValueError("source/publication count mismatch")
        fps = capture["fps"]
        seconds = len(frames) / fps
        if seconds != capture["seconds"]:
            raise ValueError("duration mismatch")
        game = {
            "game": name,
            "frames": len(frames),
            "fps": fps,
            "seconds": seconds,
            "source_sha256": sha(source),
            "framed_publications_sha256": sha(raw),
            "mean_beams": sum(len(frame["lines"]) for frame in source_frames) / len(source_frames),
            "adjacent_equal_beams": sum(a["lines"] == b["lines"] for a, b in zip(source_frames, source_frames[1:])),
            "rui": rate({"bytes": sum(map(len, frames))}, seconds),
            "transport": {},
            "beam_source": {},
            "h264": [{"width": r["width"], "height": r["height"], "codec": c["codec"], "mbps": c["bitsPerSecond"] / 1e6} for r in reference_game["resolutions"] for c in r["rasters"] if c["codec"] in ("h264-crf18-veryfast", "h264-lossless-rgb-veryfast")],
        }
        for codec, level, reset in [("zlib", 6, 1), ("zlib", 6, fps), ("zlib", 6, 0), ("zstd", 3, 1), ("zstd", 1, fps), ("zstd", 3, fps), ("zstd", 6, fps), ("zstd", 3, 0)]:
            name = f"{codec}{level}-reset{reset}"
            game["transport"][name] = rate(compress_frames(frames, codec, level, reset, repeats), seconds)
        for style in ("f64", "decimal-delta", "byte-shuffle"):
            packed, decoded = beam_arrays(source_frames, style, fps)
            entry = rate(compress_frames(packed, "zstd", 3, fps, repeats), seconds)
            entry["uncompressed_bytes"] = sum(map(len, packed))
            entry["exact_source_float_bits"] = True
            entry["exact_rui_reexport"] = verify_projector(binary, decoded, raw) if binary else None
            entry["scope"] = "line count and endpoint/color arrays only; excludes RUI records, semantic controls, protocol framing, and source control metadata"
            game["beam_source"][style] = entry
        result["games"].append(game)
    return result


def tables(result):
    games = result["games"]
    rows = ["# Offline bandwidth strategy probes", "", "Payload Mb/s on the retained eight-second, 25 Hz game traces.", "", "| Representation | Battlezone | Asteroids |", "| --- | ---: | ---: |"]
    rows.append("| Current RUI | " + " | ".join(f"{g['rui']['mbps']:.3f}" for g in games) + " |")
    for category in ("transport", "beam_source"):
        for name in games[0][category]:
            rows.append(f"| {category}: {name} | " + " | ".join(f"{g[category][name]['mbps']:.3f}" for g in games) + " |")
    for width in (320, 640, 1280):
        rows.append(f"| H.264 CRF18 veryfast, width {width} | " + " | ".join(f"{next(c['mbps'] for c in g['h264'] if c['width'] == width and c['codec'] == 'h264-crf18-veryfast'):.3f}" for g in games) + " |")
    rows.extend(["", "Transport rows reconstruct complete core publications.", "Beam rows reconstruct source geometry and omit protocol records.", "Every update is flushed immediately.", "Zstandard history is capped at 256 KiB; reset0 keeps this bounded history for the full clip.", "Reset25 starts new compression history every second; it is not a scene recovery checkpoint.", "Byte shuffling is retained as a negative result."])
    return "\n".join(rows) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, default=DEFAULT_EVIDENCE)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--game-trace", type=Path, help="Also re-export reconstructed beam sources and require byte-identical RUI envelopes")
    args = parser.parse_args()
    if args.repeats < 1:
        parser.error("--repeats must be positive")
    args.output.mkdir(parents=True, exist_ok=False)
    result = analyze(args.evidence.resolve(), args.repeats, args.game_trace.resolve() if args.game_trace else None)
    (args.output / "summary.json").write_text(json.dumps(result, indent=2) + "\n")
    (args.output / "tables.md").write_text(tables(result))
    print(tables(result))


if __name__ == "__main__":
    main()
