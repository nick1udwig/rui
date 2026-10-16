#!/usr/bin/python3
"""Normal GTK editor started independently before RUI; test-only local controls."""
import json
import os
from pathlib import Path
import sys
import gi
gi.require_version('Gtk', '4.0')
from gi.repository import Gtk, GLib

assert os.environ.get('RUI_ISOLATED_DISPLAY') == '1'
identity = sys.argv[2] if len(sys.argv) > 2 else 'A'
app = Gtk.Application(application_id='org.rui.DesktopNotesTest' + identity)


def activate(app):
    window = Gtk.ApplicationWindow(application=app, title='Existing desktop notes', default_width=500, default_height=500)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
    editor = Gtk.TextView()
    editor.update_property([Gtk.AccessibleProperty.LABEL], ['Document'])
    editor.get_buffer().set_text('Already open on the host' if identity == 'A' else 'Second existing application')
    box.append(editor)
    password = Gtk.Entry(visibility=False, text='never-publish-desktop-secret')
    box.append(password)
    protected = Gtk.Entry(visibility=False, input_purpose=Gtk.InputPurpose.PASSWORD, text='also-never-publish-desktop-secret')
    box.append(protected)
    status = Gtk.Label(label='Not saved')
    save = Gtk.Button(label='Save document')
    def saved(_):
        buffer = editor.get_buffer()
        Path(sys.argv[1]).write_text(buffer.get_text(buffer.get_start_iter(), buffer.get_end_iter(), True))
        status.set_label('Document saved')
    save.connect('clicked', saved)
    box.append(save)
    box.append(status)
    window.set_child(box)
    window.present()
    dialogs = []
    def command(fd, condition):
        line = sys.stdin.readline()
        if not line:
            app.quit()
            return False
        request = json.loads(line)
        if request['op'] == 'text':
            editor.get_buffer().set_text(request['value'])
        elif request['op'] == 'modal':
            dialog = Gtk.Window(title='Confirm action', transient_for=window, modal=True)
            button = Gtk.Button(label='Dismiss dialog')
            button.connect('clicked', lambda _: dialog.destroy())
            dialog.set_child(button)
            dialogs.append(dialog)
            dialog.present()
        elif request['op'] == 'quit':
            app.quit()
        return True
    GLib.io_add_watch(sys.stdin.fileno(), GLib.IO_IN | GLib.IO_HUP, command)
    GLib.timeout_add(300, lambda: (print(json.dumps({'ready': True, 'pid': os.getpid()}), flush=True), False)[1])

app.connect('activate', activate)
app.run([])
