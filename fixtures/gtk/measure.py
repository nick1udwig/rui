#!/usr/bin/env python3
"""Measure actual GTK frames on an isolated Xvfb or Broadway display.

Display image transport is a local test harness, never RUI. No existing desktop
session or settings are changed. GTK4.22.4 Broadway has a reproducible popup
render-node reuse assertion; Xvfb is the recommended backend for this corpus.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, default=Path('/tmp/rui-gtk-corpus'))
    parser.add_argument('--backend', choices=['xvfb', 'broadway'], default='xvfb')
    parser.add_argument('--xvfb', default=shutil.which('Xvfb'))
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    if args.backend == 'xvfb' and (not args.xvfb or not Path(args.xvfb).is_file()):
        raise SystemExit('Xvfb not installed: supply --xvfb /path/to/extracted/usr/bin/Xvfb (see docs/gtk-coverage.md)')
    with tempfile.TemporaryDirectory(prefix='rui-gtk-runtime-') as runtime:
        env = os.environ.copy()
        env.update(XDG_RUNTIME_DIR=runtime, GSK_RENDERER='cairo', GTK_A11Y='none', GSETTINGS_BACKEND='memory')
        env.pop('DISPLAY', None)
        env.pop('WAYLAND_DISPLAY', None)
        browser = None
        log = open(Path(runtime) / 'display.log', 'w+')
        if args.backend == 'xvfb':
            display = ':' + str(1000 + os.getpid() % 20_000)
            authority = runtime + '/Xauthority'
            subprocess.run(['xauth', '-f', authority, 'add', display, 'MIT-MAGIC-COOKIE-1', os.urandom(16).hex()], check=True, capture_output=True)
            env.update(GDK_BACKEND='x11', DISPLAY=display, XAUTHORITY=authority)
            command = [args.xvfb, display, '-screen', '0', '1280x1024x24', '-nolisten', 'tcp', '-auth', authority, '-noreset']
        else:
            with socket.socket() as probe:
                probe.bind(('127.0.0.1', 0))
                port = probe.getsockname()[1]
            display = str(100 + port % 20_000)
            env.update(GDK_BACKEND='broadway', BROADWAY_DISPLAY=':' + display)
            command = ['gtk4-broadwayd', '--address=127.0.0.1', '--port=' + str(port), ':' + display]
        server = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            time.sleep(.2)
            if server.poll() is not None:
                log.seek(0)
                raise RuntimeError(log.read())
            if args.backend == 'broadway':
                browser = subprocess.Popen(['chromium', '--headless', '--no-sandbox', '--disable-gpu', '--disable-background-networking', '--disable-sync', '--no-first-run', '--user-data-dir=' + runtime + '/chromium', 'http://127.0.0.1:' + str(port)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            summaries = []
            for theme in ['default', 'strict']:
                output = args.output / theme
                result = subprocess.run(['python3', str(root / 'fixtures/gtk/corpus.py'), '--theme', theme, '--output', str(output)], env=env, text=True, capture_output=True, timeout=20)
                if result.returncode or 'Traceback' in result.stderr or 'ERROR:' in result.stderr:
                    log.seek(0)
                    raise RuntimeError(f'corpus exit={result.returncode}; server exit={server.poll()}\n' + result.stdout + result.stderr + log.read())
                summary = json.loads((output / 'corpus-summary.json').read_text())
                if not summary['captures'] or not any(frame['surface'] == 'menu-popup' for frame in summary['captures']):
                    raise RuntimeError('missing actual submitted main or popup frames')
                summaries.append(summary)
                print(result.stdout.strip())
            args.output.mkdir(parents=True, exist_ok=True)
            (args.output / 'measurement.json').write_text(json.dumps({'display_harness': args.backend, 'pin': {'gtk': '4.22.4', 'cairo': '1.18.4'}, 'runs': summaries}, indent=2, sort_keys=True) + '\n')
        finally:
            if browser is not None:
                browser.terminate()
                try:
                    browser.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    browser.kill(); browser.wait()
            server.terminate(); server.wait(timeout=5)
            log.close()


if __name__ == '__main__':
    main()
