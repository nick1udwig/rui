#!/usr/bin/env python3
"""Place only explicitly named probe windows on its authenticated private Xvfb."""
import ctypes
import ctypes.util
import os
import sys


def main():
    if os.environ.get('RUI_ISOLATED_DISPLAY') != '1' or not os.environ.get('XAUTHORITY'):
        raise ValueError('authenticated private harness required')
    if len(sys.argv) != 4:
        raise ValueError('owned title and position required')
    title, px, py = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    allowed = {'RUI native GTK source controller', 'RUI native GTK source observer',
               'RUI cooperating GTK corpus', 'Controlled native modal'}
    if title not in allowed or not (0 <= px <= 3000 and 0 <= py <= 1000):
        raise ValueError('owned fixture window/placement ceiling')
    x = ctypes.CDLL(ctypes.util.find_library('X11'))
    pointer, window, number = ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint
    x.XOpenDisplay.argtypes = [ctypes.c_char_p]; x.XOpenDisplay.restype = pointer
    x.XDefaultRootWindow.argtypes = [pointer]; x.XDefaultRootWindow.restype = window
    x.XQueryTree.argtypes = [pointer, window, ctypes.POINTER(window), ctypes.POINTER(window), ctypes.POINTER(ctypes.POINTER(window)), ctypes.POINTER(number)]
    x.XFetchName.argtypes = [pointer, window, ctypes.POINTER(pointer)]
    x.XMoveWindow.argtypes = [pointer, window, ctypes.c_int, ctypes.c_int]
    x.XFree.argtypes = [pointer]; x.XFlush.argtypes = [pointer]; x.XCloseDisplay.argtypes = [pointer]
    connection = x.XOpenDisplay(os.environ['DISPLAY'].encode())
    if not connection:
        raise ValueError('private display unavailable')
    visited = 0
    matches = []
    def scan(identifier, depth=0):
        nonlocal visited
        visited += 1
        if visited > 4096 or depth > 32:
            raise ValueError('private window census ceiling')
        name = pointer()
        if x.XFetchName(connection, identifier, ctypes.byref(name)) and name.value:
            try:
                if ctypes.string_at(name.value) == title.encode():
                    matches.append(identifier)
            finally:
                x.XFree(name)
        root, parent, count, children = window(), window(), number(), ctypes.POINTER(window)()
        if x.XQueryTree(connection, identifier, ctypes.byref(root), ctypes.byref(parent), ctypes.byref(children), ctypes.byref(count)):
            try:
                if count.value > 4096:
                    raise ValueError('private window child ceiling')
                for child in children[:count.value]:
                    scan(child, depth + 1)
            finally:
                if children:
                    x.XFree(children)
    try:
        scan(x.XDefaultRootWindow(connection))
        if len(matches) != 1:
            raise ValueError('unique owned fixture window required')
        x.XMoveWindow(connection, matches[0], px, py)
        x.XFlush(connection)
    finally:
        x.XCloseDisplay(connection)


if __name__ == '__main__':
    main()
