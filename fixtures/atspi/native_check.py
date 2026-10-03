#!/usr/bin/env python3
"""Run AT-SPI evidence exclusively on an owned display and private session bus."""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--xvfb', required=True)
    parser.add_argument('--driver', type=Path, default=Path(__file__).with_name('driver.py'))
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='rui-atspi-display-') as runtime:
        display = ':' + str(1100 + os.getpid() % 20000)
        authority = runtime + '/Xauthority'
        subprocess.run(['xauth', '-f', authority, 'add', display, 'MIT-MAGIC-COOKIE-1', os.urandom(16).hex()], check=True, capture_output=True)
        env = os.environ.copy()
        env['RUI_ATSPI_OUTPUT'] = str(args.output.resolve())
        env.update(XDG_RUNTIME_DIR=runtime, RUI_ISOLATED_DISPLAY='1', GSK_RENDERER='cairo', GTK_A11Y='atspi', GSETTINGS_BACKEND='memory', GDK_BACKEND='x11', DISPLAY=display, XAUTHORITY=authority, GIO_USE_VFS='local', GTK_USE_PORTAL='0')
        for key in ('WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'AT_SPI_BUS_ADDRESS', 'NO_AT_BRIDGE'):
            env.pop(key, None)
        with open(runtime + '/display.log', 'w+') as log:
            server = subprocess.Popen([args.xvfb, display, '-screen', '0', '800x600x24', '-nolisten', 'tcp', '-auth', authority, '-noreset'], env=env, stdout=log, stderr=subprocess.STDOUT)
            try:
                time.sleep(.2)
                if server.poll() is not None:
                    raise RuntimeError('isolated Xvfb startup failed')
                result = subprocess.run(['dbus-run-session', '--', '/usr/bin/python3', str(args.driver)], env=env, text=True, capture_output=True, timeout=60)
                if result.returncode or 'Traceback' in result.stderr:
                    raise RuntimeError(result.stdout + result.stderr)
                print(result.stdout.strip())
            finally:
                server.terminate()
                server.wait(timeout=5)


if __name__ == '__main__':
    main()
