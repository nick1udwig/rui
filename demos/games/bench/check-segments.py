#!/usr/bin/env python3
"""Replay compact geometry through production compression, WASM and Canvas."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[3]
EVIDENCE = ROOT / "demos/games/evidence/comparison"


def run(*args):
    return subprocess.check_output([str(arg) for arg in args], cwd=ROOT, text=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--render", action="store_true", help="Render all frames and compare retained pixel hashes; requires Chromium and localhost.")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    results = []
    for game in ("bzone", "asteroid"):
        source = EVIDENCE / game
        target = output / game
        target.mkdir(exist_ok=True)
        frames = gzip.decompress((source / "frames.jsonl.gz").read_bytes())
        (target / "frames.jsonl").write_bytes(frames)
        (target / "capture.json").write_bytes((source / "capture.json").read_bytes())
        run(ROOT / "target/release/game-trace", target / "frames.jsonl", target / "publications.rui", target / "publications.json")
        data = (target / "publications.rui").read_bytes()
        (target / "publications.rui.gz").write_bytes(gzip.compress(data, mtime=0))
        result = {"game": game, "source_sha256": hashlib.sha256(frames).hexdigest(), "core_sha256": hashlib.sha256(data).hexdigest(), "seconds": json.loads((target / "capture.json").read_text())["seconds"]}
        for mode in ("independent", "history"):
            command = [ROOT / "target/release/examples/compression_probe", target / "publications.rui", target / (mode + ".packets")]
            if mode == "history":
                command.append("history")
            result[mode] = json.loads(run(*command))
            result[mode]["wasm"] = json.loads(run("node", "demos/games/bench/check-compression.mjs", target / "publications.rui.gz", target / (mode + ".packets")))
            result[mode]["mbps"] = result[mode]["compressed_publication_packet_bytes"] * 8 / result["seconds"] / 1e6
        if args.render:
            print(run("node", "demos/games/bench/render.mjs", target), flush=True)
        if (target / "render.json").exists():
            current = json.loads((target / "render.json").read_text())
            reference = json.loads(gzip.decompress((source / "render.json.gz").read_bytes()))
            expected = {row["width"]: row for row in reference["results"]}
            result["pixels"] = []
            for row in current["results"]:
                original = expected[row["width"]]
                assert row["frames"] == original["frames"]
                assert row["rgbaSha256"] == original["rgbaSha256"], (game, row["width"], "pixel mismatch")
                result["pixels"].append({key: row[key] for key in ("width", "height", "frames", "rgbaSha256")})
            result["renderer_environment"] = {key: current[key] for key in ("browser", "cpu", "kernel", "node", "hashes")}
        results.append(result)
        print(json.dumps(result), flush=True)
    (output / "summary.json").write_text(json.dumps({"games": results}, indent=2) + "\n")


if __name__ == "__main__":
    main()
