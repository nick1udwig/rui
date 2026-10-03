#!/usr/bin/python3
"""Run native mirror probes on an authenticated private Xvfb, never a user display."""
import argparse
import ctypes
import ctypes.util
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile
import time


def type_probe(display, authority, physical=False):
    x = ctypes.CDLL(ctypes.util.find_library('X11'))
    xtst = ctypes.CDLL(ctypes.util.find_library('Xtst'))
    x.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x.XOpenDisplay.restype = ctypes.c_void_p
    x.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
    x.XDefaultRootWindow.restype = ctypes.c_ulong
    x.XQueryTree.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.POINTER(ctypes.c_ulong)), ctypes.POINTER(ctypes.c_uint)]
    x.XFetchName.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_void_p)]
    x.XFree.argtypes = [ctypes.c_void_p]
    x.XSetInputFocus.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
    x.XStringToKeysym.argtypes = [ctypes.c_char_p]
    x.XStringToKeysym.restype = ctypes.c_ulong
    x.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
    x.XKeysymToKeycode.restype = ctypes.c_uint
    x.XFlush.argtypes = [ctypes.c_void_p]
    x.XCloseDisplay.argtypes = [ctypes.c_void_p]
    xtst.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
    previous_authority = os.environ.get('XAUTHORITY')
    os.environ['XAUTHORITY'] = authority
    try:
        connection = x.XOpenDisplay(display.encode())
    finally:
        if previous_authority is None:
            os.environ.pop('XAUTHORITY', None)
        else:
            os.environ['XAUTHORITY'] = previous_authority
    if not connection:
        raise RuntimeError('private display unavailable')
    try:
        def find(window):
            name = ctypes.c_void_p()
            if x.XFetchName(connection, window, ctypes.byref(name)) and name.value:
                try:
                    expected = b'RUI physical key probe' if physical else b'RUI native IME probe'
                    if ctypes.string_at(name.value) == expected:
                        return window
                finally:
                    x.XFree(name)
            root, parent, count = ctypes.c_ulong(), ctypes.c_ulong(), ctypes.c_uint()
            children = ctypes.POINTER(ctypes.c_ulong)()
            if x.XQueryTree(connection, window, ctypes.byref(root), ctypes.byref(parent), ctypes.byref(children), ctypes.byref(count)):
                try:
                    for child in list(children[:count.value]):
                        found = find(child)
                        if found:
                            return found
                finally:
                    if children:
                        x.XFree(children)
            return None
        window = None
        for _ in range(100):
            window = find(x.XDefaultRootWindow(connection))
            if window:
                break
            time.sleep(.02)
        if not window:
            raise RuntimeError('native probe window not found')
        x.XSetInputFocus(connection, window, 1, 0)
        x.XFlush(connection)
        time.sleep(.05)
        def event(name, pressed):
            code = x.XKeysymToKeycode(connection, x.XStringToKeysym(name.encode()))
            if not code or not xtst.XTestFakeKeyEvent(connection, code, pressed, 0):
                raise RuntimeError('private native key injection failed')
            x.XFlush(connection)
            time.sleep(.02)
        if physical:
            event('y', 1); event('y', 0)
            event('Shift_L', 1); event('y', 1)
            # Let the actual private X server emit repeat opportunities. The
            # GTK client must forward only the original physical down/up.
            time.sleep(1.2)
            event('y', 0); event('Shift_L', 0)
            return
        event('a', 1); event('a', 0)
        event('Control_L', 1); event('Shift_L', 1); event('u', 1); event('u', 0)
        event('Shift_L', 0); event('Control_L', 0)
        for name in ['0', '0', 'e', '9', 'Return']:
            event(name, 1); event(name, 0)
    finally:
        x.XCloseDisplay(connection)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--xvfb', default=shutil.which('Xvfb'))
    parser.add_argument('--probe', choices=['ime', 'physical', 'mirror', 'corpus'], required=True)
    parser.add_argument('--target-dir', type=Path, default=Path('target/debug'))
    parser.add_argument('--report', type=Path, help='optional corpus proof JSON output')
    args = parser.parse_args()
    if args.report and args.probe != 'corpus':
        parser.error('--report applies only to --probe corpus')
    if not args.xvfb or not Path(args.xvfb).is_file():
        raise SystemExit('supply the isolated Xvfb binary with --xvfb')
    with tempfile.TemporaryDirectory(prefix='rui-native-smoke-') as directory:
        display = ':' + str(1000 + os.getpid() % 20000)
        authority = str(Path(directory) / 'Xauthority')
        subprocess.run(['xauth', '-f', authority, 'add', display, 'MIT-MAGIC-COOKIE-1', secrets.token_hex(16)], check=True, capture_output=True)
        env = os.environ.copy()
        env.pop('WAYLAND_DISPLAY', None)
        env.update(DISPLAY=display, XAUTHORITY=authority, XDG_RUNTIME_DIR=directory, GDK_BACKEND='x11', GSK_RENDERER='cairo', GTK_A11Y='none', GSETTINGS_BACKEND='memory', GTK_IM_MODULE='simple', RUI_ISOLATED_DISPLAY='1')
        with (Path(directory) / 'display.log').open('w+') as log:
            server = subprocess.Popen([args.xvfb, display, '-screen', '0', '1280x1024x24', '-nolisten', 'tcp', '-auth', authority, '-noreset'], env=env, stdout=log, stderr=subprocess.STDOUT)
            child = None
            try:
                time.sleep(.2)
                if server.poll() is not None:
                    log.seek(0)
                    raise RuntimeError('private Xvfb exited: ' + log.read(8192))
                binary = args.target_dir / 'examples' / {
                    'ime': 'native_ime_probe',
                    'physical': 'native_physical_probe',
                    'mirror': 'desktop_smoke',
                    'corpus': 'corpus_smoke',
                }[args.probe]
                command = ['dbus-run-session', '--', str(binary)]
                if args.probe == 'mirror':
                    command.append(str(args.target_dir / 'rui-mirror'))
                elif args.probe == 'corpus' and args.report:
                    command.append(str(args.report.absolute()))
                child = subprocess.Popen(command, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                if args.probe in ('ime', 'physical'):
                    type_probe(display, authority, physical=args.probe == 'physical')
                stdout, stderr = child.communicate(timeout=300 if args.probe == 'corpus' else 30)
                if child.returncode:
                    raise RuntimeError(f'native probe failed: {stdout}\n{stderr}')
                print(stdout.strip())
            finally:
                if child is not None and child.poll() is None:
                    child.kill(); child.wait()
                server.terminate(); server.wait(timeout=5)


if __name__ == '__main__':
    main()
