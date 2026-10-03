#!/usr/bin/env python3
"""Run the common-ledger native GTK bridge on an isolated authenticated Xvfb."""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--xvfb', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--case', '--scenario', choices=['semantic', 'unicode', 'restart', 'whole-native', 'native-context'], default='semantic')
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='rui-gtk-native-display-') as runtime:
        display = ':' + str(1100 + os.getpid() % 20_000)
        authority = runtime + '/Xauthority'
        subprocess.run(['xauth', '-f', authority, 'add', display, 'MIT-MAGIC-COOKIE-1', os.urandom(16).hex()], check=True, capture_output=True)
        env = os.environ.copy()
        env.update(XDG_RUNTIME_DIR=runtime, GSK_RENDERER='cairo', GTK_A11Y='none', GSETTINGS_BACKEND='memory', GDK_BACKEND='x11', DISPLAY=display, XAUTHORITY=authority, GIO_USE_VFS='local', GTK_USE_PORTAL='0')
        env.pop('WAYLAND_DISPLAY', None)
        with open(runtime + '/display.log', 'w+') as log:
            server = subprocess.Popen([args.xvfb, display, '-screen', '0', '1280x1024x24', '-nolisten', 'tcp', '-auth', authority, '-noreset'], env=env, stdout=log, stderr=subprocess.STDOUT)
            try:
                time.sleep(.2)
                if server.poll() is not None:
                    log.seek(0)
                    raise RuntimeError(log.read())
                command = ['dbus-run-session', '--', 'target/debug/gtk_command_check', str(args.output)]
                if args.case != 'semantic':
                    command.append(args.case)
                result = subprocess.run(command, env=env, text=True, capture_output=True, timeout=60)
                if result.returncode or 'Traceback' in result.stderr:
                    raise RuntimeError(result.stdout + result.stderr)
                print(result.stdout.strip())
            finally:
                server.terminate()
                server.wait(timeout=5)


if __name__ == '__main__':
    main()
