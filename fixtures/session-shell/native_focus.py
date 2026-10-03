#!/usr/bin/python3
"""Finite focus/held-key actions on the runner's authenticated private Xvfb."""
import ctypes
import ctypes.util
import os
import sys


def main():
    if os.environ.get('RUI_ISOLATED_DISPLAY') != '1' or not os.environ.get('DISPLAY') or not os.environ.get('XAUTHORITY'):
        raise RuntimeError('authenticated private display required')
    if len(sys.argv) != 3 or len(sys.argv[1]) > 128 or sys.argv[2] not in ('down', 'up', 'focus'):
        raise ValueError('finite native focus action required')
    title, operation = sys.argv[1:]
    x = ctypes.CDLL(ctypes.util.find_library('X11'))
    xtst = ctypes.CDLL(ctypes.util.find_library('Xtst'))
    ptr, ulong, integer, unsigned = ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_uint
    x.XOpenDisplay.argtypes = [ctypes.c_char_p]; x.XOpenDisplay.restype = ptr
    x.XDefaultRootWindow.argtypes = [ptr]; x.XDefaultRootWindow.restype = ulong
    x.XQueryTree.argtypes = [ptr, ulong, ctypes.POINTER(ulong), ctypes.POINTER(ulong), ctypes.POINTER(ctypes.POINTER(ulong)), ctypes.POINTER(unsigned)]
    x.XFetchName.argtypes = [ptr, ulong, ctypes.POINTER(ptr)]
    x.XFree.argtypes = [ptr]
    x.XSetInputFocus.argtypes = [ptr, ulong, integer, ulong]
    x.XRaiseWindow.argtypes = [ptr, ulong]
    x.XStringToKeysym.argtypes = [ctypes.c_char_p]; x.XStringToKeysym.restype = ulong
    x.XKeysymToKeycode.argtypes = [ptr, ulong]; x.XKeysymToKeycode.restype = unsigned
    x.XSync.argtypes = [ptr, integer]; x.XCloseDisplay.argtypes = [ptr]
    xtst.XTestFakeKeyEvent.argtypes = [ptr, unsigned, integer, ulong]
    connection = x.XOpenDisplay(os.environ['DISPLAY'].encode())
    if not connection:
        raise RuntimeError('private display unavailable')
    try:
        def find(window, depth=0):
            if depth > 8:
                return None
            name = ptr()
            if x.XFetchName(connection, window, ctypes.byref(name)) and name.value:
                try:
                    if ctypes.string_at(name.value) == title.encode():
                        return window
                finally:
                    x.XFree(name)
            parent, root, count = ulong(), ulong(), unsigned()
            children = ctypes.POINTER(ulong)()
            if x.XQueryTree(connection, window, ctypes.byref(root), ctypes.byref(parent), ctypes.byref(children), ctypes.byref(count)):
                try:
                    if count.value > 128:
                        raise RuntimeError('native window ceiling')
                    for child in children[:count.value]:
                        found = find(child, depth + 1)
                        if found:
                            return found
                finally:
                    if children:
                        x.XFree(children)
            return None
        window = find(x.XDefaultRootWindow(connection))
        if not window:
            raise RuntimeError('private focus target unavailable')
        if operation != 'up':
            x.XRaiseWindow(connection, window)
            x.XSetInputFocus(connection, window, 1, 0)
        if operation != 'focus':
            code = x.XKeysymToKeycode(connection, x.XStringToKeysym(b'y'))
            if code != 29 or not xtst.XTestFakeKeyEvent(connection, code, operation == 'down', 0):
                raise RuntimeError('pinned US physical y key unavailable')
        x.XSync(connection, 0)
    finally:
        x.XCloseDisplay(connection)


if __name__ == '__main__':
    main()
