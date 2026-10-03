#!/usr/bin/env python3
"""Measure source rejection on a private authenticated Xvfb and session bus."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--xvfb', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    with tempfile.TemporaryDirectory(prefix='rui-gtk-negative-display-') as runtime:
        display = ':' + str(1200 + os.getpid() % 20_000)
        authority = runtime + '/Xauthority'
        subprocess.run(['xauth', '-f', authority, 'add', display, 'MIT-MAGIC-COOKIE-1', os.urandom(16).hex()], check=True, capture_output=True)
        env = os.environ.copy()
        env.update(XDG_RUNTIME_DIR=runtime, GSK_RENDERER='cairo', GTK_A11Y='none', GSETTINGS_BACKEND='memory', GDK_BACKEND='x11', DISPLAY=display, XAUTHORITY=authority, GIO_USE_VFS='local', GTK_USE_PORTAL='0')
        env.pop('WAYLAND_DISPLAY', None)
        with open(runtime + '/display.log', 'w+') as log:
            server = subprocess.Popen([args.xvfb, display, '-screen', '0', '640x480x24', '-nolisten', 'tcp', '-auth', authority, '-noreset'], env=env, stdout=log, stderr=subprocess.STDOUT)
            try:
                time.sleep(.2)
                if server.poll() is not None:
                    log.seek(0)
                    raise RuntimeError(log.read())
                result = subprocess.run(['dbus-run-session', '--', 'python3', str(root / 'fixtures/gtk/negative_source.py'), '--output', str(args.output)], env=env, text=True, capture_output=True, timeout=20)
                if result.returncode or 'Traceback' in result.stderr:
                    raise RuntimeError(result.stdout + result.stderr)
                report = json.loads((args.output / 'negative-report.json').read_text())
                if len(report['cases']) != 6 or report['fallback_probe']['forbidden_export_calls']:
                    raise RuntimeError('missing native source cases or forbidden fallback attempt')
                print(json.dumps({'cases': [case['case'] for case in report['cases']], 'probe': report['fallback_probe']}, sort_keys=True))
            finally:
                server.terminate()
                server.wait(timeout=5)


if __name__ == '__main__':
    main()
