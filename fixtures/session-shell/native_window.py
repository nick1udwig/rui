#!/usr/bin/python3
"""Exercise the owned source window on authenticated private Xvfb only."""
import argparse
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--xvfb', default=shutil.which('Xvfb'))
    parser.add_argument('--target-dir', type=Path, default=Path('target/debug'))
    parser.add_argument('--image', type=Path, default=Path('/tmp/rui-native-source-window.pam'))
    parser.add_argument('--report', type=Path)
    args = parser.parse_args()
    if not args.xvfb or not Path(args.xvfb).is_file():
        parser.error('the isolated Xvfb binary is required')
    with tempfile.TemporaryDirectory(prefix='rui-owned-native-') as directory:
        display = ':' + str(1000 + os.getpid() % 20000)
        authority = str(Path(directory) / 'Xauthority')
        subprocess.run(['xauth', '-f', authority, 'add', display, 'MIT-MAGIC-COOKIE-1', secrets.token_hex(16)], check=True, capture_output=True)
        env = os.environ.copy()
        env.pop('WAYLAND_DISPLAY', None)
        env.update(DISPLAY=display, XAUTHORITY=authority, XDG_RUNTIME_DIR=directory, GDK_BACKEND='x11', GSK_RENDERER='cairo', GTK_A11Y='none', GSETTINGS_BACKEND='memory', RUI_ISOLATED_DISPLAY='1')
        with (Path(directory) / 'display.log').open('w+') as log:
            server = subprocess.Popen([args.xvfb, display, '-screen', '0', '1280x1024x24', '-nolisten', 'tcp', '-auth', authority, '-noreset'], env=env, stdout=log, stderr=subprocess.STDOUT)
            child = None
            try:
                time.sleep(.2)
                if server.poll() is not None:
                    log.seek(0)
                    raise RuntimeError('private Xvfb exited: ' + log.read(8192))
                child = subprocess.Popen(['dbus-run-session', '--', str(args.target_dir / 'examples/source_window_probe'), str(args.image.absolute())], env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                stdout, stderr = child.communicate(timeout=30)
                if child.returncode:
                    raise RuntimeError(f'native source probe failed: {stdout}\n{stderr}')
                reports = [json.loads(line) for line in stdout.splitlines() if line.startswith('{')]
                if len(reports) != 2:
                    raise RuntimeError('source paint and native cleanup reports are required')
                if args.report:
                    args.report.write_text(json.dumps({'source_window': reports[0], 'native_owner': reports[1]}, indent=2) + '\n')
                print(stdout.strip())
            finally:
                if child is not None and child.poll() is None:
                    child.kill(); child.wait()
                server.terminate(); server.wait(timeout=5)


if __name__ == '__main__':
    main()
