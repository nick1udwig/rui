#!/usr/bin/env python3
"""Capture a fixed-rate native game workload on an owned display."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))


def mask_at(t, game):
    if t < 0.25:
        return 0
    if t < 0.50:
        return 32
    if t < 0.75:
        return 0
    if t < 1.0:
        return 32 if game == 'bzone' else 0
    if t < 1.25:
        return 0
    if t < 1.50:
        return 64
    if t < 2.0:
        return 0
    phase = int((t - 2.0) / 2.0) % 4
    if game == 'bzone':
        return [13, 5, 14, 0][phase]
    return [12, 5, 6, 0][phase]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mame', required=True)
    parser.add_argument('--rompath', required=True, type=Path)
    parser.add_argument('--game', choices=['bzone', 'asteroid'], required=True)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--xvfb', type=Path)
    parser.add_argument('--seconds', type=int, default=8)
    parser.add_argument('--fps', type=int, default=25)
    parser.add_argument('--inside-display', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not 4 <= args.seconds <= 30 or not 1 <= args.fps <= 30:
        parser.error('seconds must be 4..30 and fps must be 1..30')
    if args.seconds * args.fps < 10:
        parser.error('capture needs at least 10 frames for warm statistics')
    if not args.inside_display:
        if not args.xvfb:
            parser.error('--xvfb is required')
        from fixtures.gtk.mirror_check import run_private
        print(run_private(str(args.xvfb.resolve()), [sys.executable, str(Path(__file__).resolve()), *sys.argv[1:], '--inside-display'], timeout=args.seconds + 45).strip())
        return
    if os.environ.get('RUI_ISOLATED_DISPLAY') != '1':
        parser.error('owned display required')
    args.output.mkdir(parents=True, exist_ok=False)
    with tempfile.TemporaryDirectory(prefix='rui-native-bench-') as temporary:
        native = Path(temporary)
        environment = dict(os.environ, RUI_GAME=args.game, RUI_GAME_STATE=str(native))
        command = [args.mame, args.game, '-rompath', str(args.rompath.resolve()), '-noreadconfig', '-noplugins', '-skip_gameinfo', '-window', '-video', 'soft', '-sound', 'none', '-resolution', '640x480', '-autoboot_delay', '0', '-autoboot_script', str(ROOT/'demos/games/mame.lua')]
        for option in ['homepath', 'cfg_directory', 'nvram_directory', 'input_directory', 'snapshot_directory']:
            command += ['-' + option, str(native)]
        log = (args.output/'mame.log').open('w')
        child = subprocess.Popen(command, env=environment, stdout=log, stderr=log)
        def interrupted(*_):
            raise KeyboardInterrupt
        signal.signal(signal.SIGTERM, interrupted)
        frames, revision = [], 0
        try:
            deadline = time.monotonic() + 20
            while not (native/'frame.json').exists():
                if child.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError((args.output/'mame.log').read_text())
                time.sleep(.01)
            start = time.monotonic()
            for index in range((args.seconds + 2) * args.fps):
                target = start + index / args.fps
                time.sleep(max(0, target - time.monotonic()))
                if child.poll() is not None:
                    raise RuntimeError('MAME exited during capture')
                mask = mask_at(index / args.fps, args.game)
                revision += 1
                (native/'inputs.tmp').write_text(f'{int(time.time()) + 2} {mask} {revision}\n')
                (native/'inputs.tmp').replace(native/'inputs')
                if index >= 2 * args.fps:
                    frame = json.loads((native/'frame.json').read_text())
                    frame['sample'] = len(frames)
                    frames.append(frame)
            assert len(frames) == args.seconds * args.fps
            assert max(max(line[:4]) for frame in frames for line in frame['lines']) > 100
            trace = ''.join(json.dumps(frame, separators=(',', ':')) + '\n' for frame in frames).encode()
            (args.output/'frames.jsonl').write_bytes(trace)
            manifest = {'game': args.game, 'fps': args.fps, 'seconds': args.seconds, 'frames': len(frames), 'trace_sha256': hashlib.sha256(trace).hexdigest(), 'unique_native_frames': len({f['sequence'] for f in frames}), 'min_lines': min(len(f['lines']) for f in frames), 'max_lines': max(len(f['lines']) for f in frames), 'mame': subprocess.check_output([args.mame, '-version'], text=True).strip(), 'schedule': '2-second coin/start setup; repeat 2-second drive/fire, two turning/fire patterns, idle phases', 'scheduled_start_after_setup': True, 'scriptedPhaseMasks': [13,5,14,0] if args.game == 'bzone' else [12,5,6,0]}
            (args.output/'capture.json').write_text(json.dumps(manifest, indent=2) + '\n')
            print(json.dumps(manifest))
        finally:
            child.terminate()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
            log.close()


if __name__ == '__main__':
    main()
