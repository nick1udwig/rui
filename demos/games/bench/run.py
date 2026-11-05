#!/usr/bin/env python3
"""Run the paired vector/raster benchmark from native capture or retained fixtures."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
BENCH = ROOT/'demos/games/bench'


def run(*command, env=None):
    subprocess.run(command, check=True, cwd=ROOT, env=env)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True, help='new output directory')
    parser.add_argument('--fixture', type=Path, help='retained comparison fixture directory; no ROMs needed')
    parser.add_argument('--mame')
    parser.add_argument('--rompath', type=Path)
    parser.add_argument('--xvfb', type=Path)
    parser.add_argument('--chromium', default='/usr/bin/chromium')
    parser.add_argument('--seconds', type=int, default=8)
    parser.add_argument('--keep-raw', action='store_true')
    args = parser.parse_args()
    if not args.fixture and not (args.mame and args.rompath and args.xvfb):
        parser.error('supply --fixture, or --mame, --rompath, and --xvfb')
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    environment = dict(os.environ, RUI_WASM_CHROMIUM=args.chromium)
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--offline', '--no-deps', '--format-version', '1'], cwd=ROOT))
    exporter = str(Path(metadata['target_directory'])/'release/game-trace')
    for game in ['bzone', 'asteroid']:
        directory = args.output/game
        if args.fixture:
            directory.mkdir()
            source = args.fixture.resolve()/game
            (directory/'frames.jsonl').write_bytes(gzip.decompress((source/'frames.jsonl.gz').read_bytes()))
            (directory/'capture.json').write_bytes((source/'capture.json').read_bytes())
        else:
            run(sys.executable, str(BENCH/'capture.py'), '--game', game, '--mame', args.mame, '--rompath', str(args.rompath.resolve()), '--xvfb', str(args.xvfb.resolve()), '--seconds', str(args.seconds), '--output', str(directory))
        capture = json.loads((directory/'capture.json').read_text())
        if capture['game'] != game:
            raise ValueError('capture game mismatch')
        if hashlib.sha256((directory/'frames.jsonl').read_bytes()).hexdigest() != capture['trace_sha256']:
            raise ValueError('capture trace hash mismatch')
        run(exporter, str(directory/'frames.jsonl'), str(directory/'publications.rui'), str(directory/'publications.json'))
        run('node', str(BENCH/'render.mjs'), str(directory), env=environment)
        run(sys.executable, str(BENCH/'codecs.py'), str(directory))
        run('node', str(BENCH/'decode.mjs'), str(directory), env=environment)
        if not args.keep_raw:
            for raw in directory.glob('*.rgba'):
                raw.unlink()
    run(sys.executable, str(BENCH/'summarize.py'), str(args.output))


if __name__ == '__main__':
    main()
