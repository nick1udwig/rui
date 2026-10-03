#!/usr/bin/python3
"""Finite local middle-button pan on the runner's owned mirror only."""
import ctypes
import ctypes.util
import math
import os
import sys
import time


def main():
    if os.environ.get('RUI_ISOLATED_DISPLAY') != '1' or not os.environ.get('DISPLAY') or not os.environ.get('XAUTHORITY'):
        raise RuntimeError('authenticated private display required')
    if len(sys.argv) != 5:
        raise ValueError('exactly two bounded local pan points required')
    points = [float(value) for value in sys.argv[1:]]
    if any(not math.isfinite(value) or not 0 <= value < 2000 for value in points):
        raise ValueError('private local pan coordinates outside bound')
    x = ctypes.CDLL(ctypes.util.find_library('X11'))
    xt = ctypes.CDLL(ctypes.util.find_library('Xtst'))
    ptr, ulong, integer, unsigned = ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_uint
    x.XOpenDisplay.argtypes = [ctypes.c_char_p]; x.XOpenDisplay.restype = ptr
    x.XDefaultRootWindow.argtypes = [ptr]; x.XDefaultRootWindow.restype = ulong
    x.XQueryTree.argtypes = [ptr, ulong, ctypes.POINTER(ulong), ctypes.POINTER(ulong), ctypes.POINTER(ctypes.POINTER(ulong)), ctypes.POINTER(unsigned)]
    x.XFetchName.argtypes = [ptr, ulong, ctypes.POINTER(ptr)]; x.XFree.argtypes = [ptr]
    x.XTranslateCoordinates.argtypes = [ptr, ulong, ulong, integer, integer, ctypes.POINTER(integer), ctypes.POINTER(integer), ctypes.POINTER(ulong)]
    x.XGetGeometry.argtypes = [ptr, ulong, ctypes.POINTER(ulong), ctypes.POINTER(integer), ctypes.POINTER(integer), ctypes.POINTER(unsigned), ctypes.POINTER(unsigned), ctypes.POINTER(unsigned), ctypes.POINTER(unsigned)]
    x.XSetInputFocus.argtypes = [ptr, ulong, integer, ulong]
    x.XSync.argtypes = [ptr, integer]; x.XCloseDisplay.argtypes = [ptr]
    xt.XTestFakeMotionEvent.argtypes = [ptr, integer, integer, integer, ulong]
    xt.XTestFakeButtonEvent.argtypes = [ptr, unsigned, integer, ulong]
    connection = x.XOpenDisplay(os.environ['DISPLAY'].encode())
    if not connection:
        raise RuntimeError('owned display unavailable')
    held = False
    try:
        root = x.XDefaultRootWindow(connection)
        visited = 0
        def find(window, depth=0):
            nonlocal visited
            visited += 1
            if visited > 256 or depth > 8:
                raise RuntimeError('private window tree bound')
            name = ptr()
            if x.XFetchName(connection, window, ctypes.byref(name)) and name.value:
                try:
                    if ctypes.string_at(name.value) == b'RUI native session controller':
                        return window
                finally:
                    x.XFree(name)
            actual_root, parent, count = ulong(), ulong(), unsigned()
            children = ctypes.POINTER(ulong)()
            if x.XQueryTree(connection, window, ctypes.byref(actual_root), ctypes.byref(parent), ctypes.byref(children), ctypes.byref(count)):
                try:
                    if count.value > 128:
                        raise RuntimeError('private child count bound')
                    for child in children[:count.value]:
                        result = find(child, depth + 1)
                        if result:
                            return result
                finally:
                    if children:
                        x.XFree(children)
            return None
        window = find(root)
        if not window:
            raise RuntimeError('owned controller missing')
        actual_root, dx, dy, width, height, border, depth = ulong(), integer(), integer(), unsigned(), unsigned(), unsigned(), unsigned()
        if not x.XGetGeometry(connection, window, ctypes.byref(actual_root), ctypes.byref(dx), ctypes.byref(dy), ctypes.byref(width), ctypes.byref(height), ctypes.byref(border), ctypes.byref(depth)):
            raise RuntimeError('owned geometry unavailable')
        if any(points[index] >= (width.value if index % 2 == 0 else height.value) for index in range(4)):
            raise ValueError('pan outside owned mirror')
        child = ulong()
        if not x.XTranslateCoordinates(connection, window, root, 0, 0, ctypes.byref(dx), ctypes.byref(dy), ctypes.byref(child)):
            raise RuntimeError('owned translation unavailable')
        x.XSetInputFocus(connection, window, 1, 0)
        for index in range(2):
            if not xt.XTestFakeMotionEvent(connection, -1, dx.value + round(points[index * 2]), dy.value + round(points[index * 2 + 1]), 0):
                raise RuntimeError('owned pan motion failed')
            if index == 0:
                if not xt.XTestFakeButtonEvent(connection, 2, 1, 0):
                    raise RuntimeError('owned middle press failed')
                held = True
            x.XSync(connection, 0)
            time.sleep(.05)
    finally:
        if held:
            xt.XTestFakeButtonEvent(connection, 2, 0, 0)
            x.XSync(connection, 0)
        x.XCloseDisplay(connection)


if __name__ == '__main__':
    main()
