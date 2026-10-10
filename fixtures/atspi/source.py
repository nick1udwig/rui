#!/usr/bin/python3
"""Controlled real GTK application; mutations use an inherited private socket."""
import argparse
import json
import os
import socket
import gi
gi.require_version('Gtk', '4.0')
from gi.repository import Gtk, GLib


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--fd', type=int, required=True)
    parser.add_argument('--identity', choices=('A', 'B'), required=True)
    args = parser.parse_args()
    if os.environ.get('RUI_ISOLATED_DISPLAY') != '1':
        raise ValueError('owned private display required')
    channel = socket.socket(fileno=args.fd)
    channel.setblocking(False)
    app = Gtk.Application(application_id='org.rui.AtspiFixture' + args.identity)

    def activate(app):
        app.hold()
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        rows = [Gtk.Button(label=f'Row {index}') for index in range(12)]
        for row in rows:
            box.append(row)
        private = Gtk.Entry(text='never export this secret', visibility=False)
        box.append(private)
        window = Gtk.ApplicationWindow(application=app, title='Identical overlapping title', child=box,
                                       default_width=240, default_height=480)
        window.present()
        generation = [0]

        def answer(message):
            channel.send(json.dumps(message).encode())
            return False

        def command(fd, condition):
            if condition & (GLib.IO_HUP | GLib.IO_ERR):
                app.quit()
                return False
            data = channel.recv(4097)
            if not data or len(data) > 4096:
                app.quit()
                return False
            request = json.loads(data)
            if request.get('op') == 'replace':
                old = rows.pop()
                box.remove(old)
                generation[0] += 1
                row = Gtk.Button(label='Replacement ' + str(generation[0]))
                rows.append(row)
                box.append(row)
            elif request.get('op') == 'quit':
                app.quit()
                return False
            else:
                raise ValueError('unknown controlled source operation')
            GLib.timeout_add(30, answer, {'generation': generation[0]})
            return True

        GLib.io_add_watch(channel.fileno(), GLib.IO_IN | GLib.IO_HUP | GLib.IO_ERR, command)
        GLib.timeout_add(150, answer, {'ready': True, 'pid': os.getpid()})
        GLib.timeout_add_seconds(45, lambda: app.quit())
    app.connect('activate', activate)
    app.run(None)


if __name__ == '__main__':
    main()
