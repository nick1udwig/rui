#!/usr/bin/python3
"""Finite button transitions on the runner's authenticated private Xvfb."""
import argparse
import ctypes
import ctypes.util
import math
import os


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=('down', 'up'))
    parser.add_argument('x', type=float)
    parser.add_argument('y', type=float)
    args = parser.parse_args()
    if os.environ.get('RUI_ISOLATED_DISPLAY') != '1' or not os.environ.get('DISPLAY') or not os.environ.get('XAUTHORITY'):
        raise RuntimeError('authenticated private display required')
    if not all(math.isfinite(v) and 0 <= v < 2000 for v in (args.x, args.y)):
        raise ValueError('bounded private pointer coordinates required')
    x = ctypes.CDLL(ctypes.util.find_library('X11'))
    xtst = ctypes.CDLL(ctypes.util.find_library('Xtst'))
    ptr, ulong, integer, unsigned = ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_uint
    x.XOpenDisplay.argtypes = [ctypes.c_char_p]; x.XOpenDisplay.restype = ptr
    x.XDefaultRootWindow.argtypes = [ptr]; x.XDefaultRootWindow.restype = ulong
    x.XQueryTree.argtypes = [ptr, ulong, ctypes.POINTER(ulong), ctypes.POINTER(ulong), ctypes.POINTER(ctypes.POINTER(ulong)), ctypes.POINTER(unsigned)]
    x.XFetchName.argtypes = [ptr, ulong, ctypes.POINTER(ptr)]
    x.XFree.argtypes = [ptr]
    x.XGetGeometry.argtypes = [ptr, ulong, ctypes.POINTER(ulong), ctypes.POINTER(integer), ctypes.POINTER(integer), ctypes.POINTER(unsigned), ctypes.POINTER(unsigned), ctypes.POINTER(unsigned), ctypes.POINTER(unsigned)]
    x.XTranslateCoordinates.argtypes = [ptr, ulong, ulong, integer, integer, ctypes.POINTER(integer), ctypes.POINTER(integer), ctypes.POINTER(ulong)]
    x.XSync.argtypes = [ptr, integer]; x.XCloseDisplay.argtypes = [ptr]
    xtst.XTestFakeMotionEvent.argtypes = [ptr, integer, integer, integer, ulong]
    xtst.XTestFakeButtonEvent.argtypes = [ptr, unsigned, integer, ulong]
    connection = x.XOpenDisplay(os.environ['DISPLAY'].encode())
    if not connection:
        raise RuntimeError('private display unavailable')
    try:
        visited = 0
        def find(window, depth=0):
            nonlocal visited
            visited += 1
            if visited > 256 or depth > 8:
                raise RuntimeError('private window tree ceiling')
            name = ptr()
            if x.XFetchName(connection, window, ctypes.byref(name)) and name.value:
                try:
                    if ctypes.string_at(name.value) == b'RUI native session controller':
                        return window
                finally:
                    x.XFree(name)
            root, parent, count = ulong(), ulong(), unsigned()
            children = ctypes.POINTER(ulong)()
            if x.XQueryTree(connection, window, ctypes.byref(root), ctypes.byref(parent), ctypes.byref(children), ctypes.byref(count)):
                try:
                    if count.value > 128:
                        raise RuntimeError('private child count ceiling')
                    for child in children[:count.value]:
                        found = find(child, depth + 1)
                        if found:
                            return found
                finally:
                    if children:
                        x.XFree(children)
            return None
        root = x.XDefaultRootWindow(connection)
        window = find(root)
        if not window:
            raise RuntimeError('owned controller unavailable')
        gx, gy, width, height, border, depth, parent = integer(), integer(), unsigned(), unsigned(), unsigned(), unsigned(), ulong()
        if not x.XGetGeometry(connection, window, ctypes.byref(parent), ctypes.byref(gx), ctypes.byref(gy), ctypes.byref(width), ctypes.byref(height), ctypes.byref(border), ctypes.byref(depth)) or args.x >= width.value or args.y >= height.value:
            raise RuntimeError('point outside owned controller')
        if args.operation == 'down':
            px, py, child = integer(), integer(), ulong()
            if not x.XTranslateCoordinates(connection, window, root, round(args.x), round(args.y), ctypes.byref(px), ctypes.byref(py), ctypes.byref(child)) or not xtst.XTestFakeMotionEvent(connection, -1, px.value, py.value, 0):
                raise RuntimeError('private pointer placement failed')
        if not xtst.XTestFakeButtonEvent(connection, 1, args.operation == 'down', 0):
            raise RuntimeError('private button transition failed')
        x.XSync(connection, 0)
    finally:
        x.XCloseDisplay(connection)


if __name__ == '__main__':
    main()
