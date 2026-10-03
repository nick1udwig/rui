#!/usr/bin/python3
"""Run two real TLS mirrors against their owned EIS source on private Xvfb."""
import argparse
import ctypes
import ctypes.util
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile
import time


def arrange():
    """Give all three owned windows visible, disjoint GTK paint regions."""
    if os.environ.get('RUI_ISOLATED_DISPLAY') != '1' or not os.environ.get('DISPLAY') or not os.environ.get('XAUTHORITY'):
        raise RuntimeError('authenticated private display required')
    x = ctypes.CDLL(ctypes.util.find_library('X11'))
    ptr, ulong, integer, unsigned = ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_uint
    x.XOpenDisplay.argtypes = [ctypes.c_char_p]; x.XOpenDisplay.restype = ptr
    x.XDefaultRootWindow.argtypes = [ptr]; x.XDefaultRootWindow.restype = ulong
    x.XQueryTree.argtypes = [ptr, ulong, ctypes.POINTER(ulong), ctypes.POINTER(ulong), ctypes.POINTER(ctypes.POINTER(ulong)), ctypes.POINTER(unsigned)]
    x.XFetchName.argtypes = [ptr, ulong, ctypes.POINTER(ptr)]
    x.XFree.argtypes = [ptr]
    x.XMoveResizeWindow.argtypes = [ptr, ulong, integer, integer, unsigned, unsigned]
    x.XSync.argtypes = [ptr, integer]; x.XCloseDisplay.argtypes = [ptr]
    connection = x.XOpenDisplay(os.environ['DISPLAY'].encode())
    if not connection:
        raise RuntimeError('private display unavailable')
    placements = {
        b'RUI native session controller': (0, 0, 860, 560),
        b'RUI native session observer': (900, 0, 860, 560),
        b'RUI isolated source probe': (0, 650, 650, 450),
    }
    try:
        visited = 0
        def windows(window, depth=0):
            nonlocal visited
            visited += 1
            if visited > 256:
                raise RuntimeError('private total window count ceiling')
            if depth > 8:
                raise RuntimeError('private window tree depth ceiling')
            found = {}
            name = ptr()
            if x.XFetchName(connection, window, ctypes.byref(name)) and name.value:
                try:
                    title = ctypes.string_at(name.value)
                    if len(title) > 128:
                        raise RuntimeError('private window title ceiling')
                    if title in placements:
                        found[title] = window
                finally:
                    x.XFree(name)
            root, parent, count = ulong(), ulong(), unsigned()
            children = ctypes.POINTER(ulong)()
            if x.XQueryTree(connection, window, ctypes.byref(root), ctypes.byref(parent), ctypes.byref(children), ctypes.byref(count)):
                try:
                    if count.value > 128:
                        raise RuntimeError('private window count ceiling')
                    for child in children[:count.value]:
                        found.update(windows(child, depth + 1))
                finally:
                    if children:
                        x.XFree(children)
            return found
        deadline = time.monotonic() + 5
        while True:
            visited = 0
            found = windows(x.XDefaultRootWindow(connection))
            if placements.keys() <= found.keys():
                break
            if time.monotonic() >= deadline:
                raise RuntimeError('private source/mirror windows unavailable')
            time.sleep(.02)
        for title, geometry in placements.items():
            x.XMoveResizeWindow(connection, found[title], *geometry)
        x.XSync(connection, 0)
    finally:
        x.XCloseDisplay(connection)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--xvfb', default=shutil.which('Xvfb'))
    parser.add_argument('--target-dir', type=Path, default=Path('target/debug'))
    parser.add_argument('--report', type=Path, default=Path('/tmp/rui-native-session.json'))
    parser.add_argument('--arrange', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.arrange:
        arrange()
        return
    if not args.xvfb or not Path(args.xvfb).is_file():
        parser.error('an isolated Xvfb binary is required')
    binary = args.target_dir / 'examples/native_session_smoke'
    if not binary.is_file():
        parser.error('build rui-session-shell --features native-mirrors --example native_session_smoke first')
    with tempfile.TemporaryDirectory(prefix='rui-native-session-') as directory:
        display = ':' + str(1000 + os.getpid() % 20000)
        authority = str(Path(directory) / 'Xauthority')
        subprocess.run(['xauth', '-f', authority, 'add', display, 'MIT-MAGIC-COOKIE-1', secrets.token_hex(16)], check=True, capture_output=True)
        env = os.environ.copy()
        env.pop('WAYLAND_DISPLAY', None)
        env.update(DISPLAY=display, XAUTHORITY=authority, XDG_RUNTIME_DIR=directory,
                   GDK_BACKEND='x11', GSK_RENDERER='cairo', GTK_A11Y='none',
                   GSETTINGS_BACKEND='memory', RUI_ISOLATED_DISPLAY='1')
        with (Path(directory) / 'display.log').open('w+') as display_log, (Path(directory) / 'native.log').open('w+') as native_log:
            server = subprocess.Popen([args.xvfb, display, '-screen', '0', '1800x1200x24', '-nolisten', 'tcp', '-auth', authority, '-noreset'], env=env, stdout=display_log, stderr=subprocess.STDOUT)
            child = None
            try:
                time.sleep(.2)
                if server.poll() is not None:
                    display_log.seek(0)
                    raise RuntimeError('private Xvfb exited: ' + display_log.read(8192))
                subprocess.run(['setxkbmap', '-layout', 'us', '-model', 'pc105'], env=env, check=True, capture_output=True, timeout=5)
                child = subprocess.Popen(['dbus-run-session', '--', str(binary), str(args.report.absolute())], env=env, stdout=native_log, stderr=subprocess.STDOUT)
                child.wait(timeout=90)
                native_log.seek(0)
                output = native_log.read(65536)
                if child.returncode:
                    raise RuntimeError('actual native TLS/mirror gate failed:\n' + output)
                if args.report.stat().st_size > 65536:
                    raise RuntimeError('native report size ceiling')
                report = json.loads(args.report.read_text())
                if report['native_press_release_frames'] != 2 or report['host_layout'] != 'de' or report['client_layout'] != 'us':
                    raise RuntimeError('native key ownership report mismatch')
                print(output.strip())
                print('hash-bound native session report: ' + str(args.report))
            finally:
                if child is not None and child.poll() is None:
                    child.kill(); child.wait()
                server.terminate(); server.wait(timeout=5)


if __name__ == '__main__':
    main()
