#!/usr/bin/python3
"""Bounded XTest actions for named windows on the harness-owned private display."""
import ctypes
import ctypes.util
import os
import sys
import time


def main():
    if os.environ.get('RUI_ISOLATED_DISPLAY') != '1':
        raise RuntimeError('private harness display required')
    if not os.environ.get('DISPLAY') or not os.environ.get('XAUTHORITY'):
        raise RuntimeError('authenticated private display required')
    args = sys.argv[1:]
    if not 2 <= len(args) <= 130 or len(args[0]) > 128:
        raise ValueError('native action ceiling')
    title, operation = args[:2]
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
    x.XTranslateCoordinates.argtypes = [ptr, ulong, ulong, integer, integer, ctypes.POINTER(integer), ctypes.POINTER(integer), ctypes.POINTER(ulong)]
    x.XGetGeometry.argtypes = [ptr, ulong, ctypes.POINTER(ulong), ctypes.POINTER(integer), ctypes.POINTER(integer), ctypes.POINTER(unsigned), ctypes.POINTER(unsigned), ctypes.POINTER(unsigned), ctypes.POINTER(unsigned)]
    x.XResizeWindow.argtypes = [ptr, ulong, unsigned, unsigned]
    x.XFlush.argtypes = [ptr]; x.XCloseDisplay.argtypes = [ptr]
    xtst.XTestFakeKeyEvent.argtypes = [ptr, unsigned, integer, ulong]
    xtst.XTestFakeButtonEvent.argtypes = [ptr, unsigned, integer, ulong]
    xtst.XTestFakeMotionEvent.argtypes = [ptr, integer, integer, integer, ulong]
    connection = x.XOpenDisplay(os.environ['DISPLAY'].encode())
    if not connection:
        raise RuntimeError('private display unavailable')
    try:
        root = x.XDefaultRootWindow(connection)

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
            parent, actual_root, count = ulong(), ulong(), unsigned()
            children = ctypes.POINTER(ulong)()
            if x.XQueryTree(connection, window, ctypes.byref(actual_root), ctypes.byref(parent), ctypes.byref(children), ctypes.byref(count)):
                try:
                    if count.value > 128:
                        raise RuntimeError('private window count ceiling')
                    for child in list(children[:count.value]):
                        found = find(child, depth + 1)
                        if found:
                            return found
                finally:
                    if children:
                        x.XFree(children)
            return None

        window = None
        for _ in range(100):
            window = find(root)
            if window:
                break
            time.sleep(.02)
        if not window:
            raise RuntimeError('named private mirror window unavailable')
        x.XRaiseWindow(connection, window)
        x.XSetInputFocus(connection, window, 1, 0)
        x.XFlush(connection)

        def key(name, pressed):
            if len(name) > 32 or not name.isascii():
                raise ValueError('native key name ceiling')
            code = x.XKeysymToKeycode(connection, x.XStringToKeysym(name.encode()))
            if not code or not xtst.XTestFakeKeyEvent(connection, code, pressed, 0):
                raise RuntimeError('private native key injection failed')

        if operation == 'keys':
            held = set()
            try:
                for name in args[2:]:
                    if name.startswith('+'):
                        held.add(name[1:]); key(name[1:], 1)
                    elif name.startswith('-'):
                        key(name[1:], 0); held.discard(name[1:])
                    else:
                        key(name, 1); key(name, 0)
            finally:
                for name in held:
                    key(name, 0)
        elif operation == 'click' and len(args) == 4:
            px, py = map(float, args[2:])
            if not (0 <= px <= 4096 and 0 <= py <= 4096):
                raise ValueError('native click coordinate ceiling')
            dx, dy, child = integer(), integer(), ulong()
            if not x.XTranslateCoordinates(connection, window, root, 0, 0, ctypes.byref(dx), ctypes.byref(dy), ctypes.byref(child)):
                raise RuntimeError('private window translation unavailable')
            if not xtst.XTestFakeMotionEvent(connection, -1, dx.value + round(px), dy.value + round(py), 0):
                raise RuntimeError('private motion injection failed')
            if not xtst.XTestFakeButtonEvent(connection, 1, 1, 0) or not xtst.XTestFakeButtonEvent(connection, 1, 0, 0):
                raise RuntimeError('private button injection failed')
        elif operation in ('drag', 'drag_cancel') and 6 <= len(args) <= 130 and len(args) % 2 == 0:
            points = [(float(args[i]), float(args[i + 1])) for i in range(2, len(args), 2)]
            if any(not (0 <= px <= 4096 and 0 <= py <= 4096) for px, py in points):
                raise ValueError('native drag coordinate ceiling')
            dx, dy, child = integer(), integer(), ulong()
            if not x.XTranslateCoordinates(connection, window, root, 0, 0, ctypes.byref(dx), ctypes.byref(dy), ctypes.byref(child)):
                raise RuntimeError('private window translation unavailable')
            pressed = False
            try:
                for index, (px, py) in enumerate(points):
                    if not xtst.XTestFakeMotionEvent(connection, -1, dx.value + round(px), dy.value + round(py), 0):
                        raise RuntimeError('private drag motion failed')
                    if index == 0:
                        if not xtst.XTestFakeButtonEvent(connection, 1, 1, 0):
                            raise RuntimeError('private drag press failed')
                        pressed = True
                    x.XFlush(connection)
                    # Give GDK an opportunity to capture each injected sample.
                    # The Rust oracle compares every actually captured callback,
                    # rather than assuming XTest guarantees toolkit delivery.
                    time.sleep(.025)
                if operation == 'drag_cancel':
                    key('Escape', 1); key('Escape', 0)
                    x.XFlush(connection)
                    time.sleep(.05)
            finally:
                if pressed:
                    xtst.XTestFakeButtonEvent(connection, 1, 0, 0)
                    x.XFlush(connection)
        elif operation == 'resize_by' and len(args) == 4:
            width_delta, height_delta = map(int, args[2:])
            actual_root, px, py = ulong(), integer(), integer()
            width, height, border, depth = unsigned(), unsigned(), unsigned(), unsigned()
            if not x.XGetGeometry(connection, window, ctypes.byref(actual_root), ctypes.byref(px), ctypes.byref(py), ctypes.byref(width), ctypes.byref(height), ctypes.byref(border), ctypes.byref(depth)):
                raise RuntimeError('private window geometry unavailable')
            width, height = width.value + width_delta, height.value + height_delta
            if not (64 <= width <= 2048 and 64 <= height <= 1024):
                raise ValueError('native resize ceiling')
            x.XResizeWindow(connection, window, width, height)
        else:
            raise ValueError('unknown native action')
        x.XFlush(connection)
    finally:
        x.XCloseDisplay(connection)


if __name__ == '__main__':
    main()
