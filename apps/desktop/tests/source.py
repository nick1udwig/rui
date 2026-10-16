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
    extra_windows = []
    control_widgets = {}
    def command(fd, condition):
        line = sys.stdin.readline()
        if not line:
            app.quit()
            return False
        request = json.loads(line)
        if request['op'] == 'text':
            editor.get_buffer().set_text(request['value'])
        elif request['op'] == 'sibling':
            box.append(Gtk.Label(label='Unrelated new status object'))
        elif request['op'] == 'window':
            extra = Gtk.ApplicationWindow(application=app, title='Second notes window')
            extra.set_child(Gtk.Label(label='Content in the second window'))
            extra_windows.append(extra)
            extra.present()
        elif request['op'] == 'close-window':
            if extra_windows: extra_windows.pop().destroy()
        elif request['op'] == 'reorder-items':
            items = control_widgets['items']
            row = items.get_row_at_index(1)
            items.remove(row)
            items.prepend(row)
        elif request['op'] == 'too-deep':
            extra = Gtk.ApplicationWindow(application=app, title='Bounded deep tree')
            top = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
            current = top
            for _ in range(70):
                child = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
                current.append(child)
                current = child
            current.append(Gtk.Label(label='Beyond the supported depth'))
            extra.set_child(top)
            extra_windows.append(extra)
            extra.present()
        elif request['op'] == 'controls':
            controls = Gtk.ApplicationWindow(application=app, title='Semantic controls', default_width=500, default_height=600)
            panel = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
            completed = [False]
            menu_button = Gtk.MenuButton(label='Actions')
            popover = Gtk.Popover()
            mark = Gtk.Button(label='Mark done')
            def mark_done(*_):
                completed[0] = True
                recorded()
                menu_button.popdown()
            mark.connect('clicked', mark_done)
            popover.set_child(mark)
            menu_button.set_popover(popover)
            panel.append(menu_button)
            panel.append(Gtk.Button())
            deep = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
            panel.append(deep)
            for _ in range(32):
                child = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
                deep.append(child)
                deep = child
            deep.append(Gtk.Label(label='Deeply nested content'))
            volume = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, 0, 100, .5)
            volume.update_property([Gtk.AccessibleProperty.LABEL], ['Volume'])
            volume.set_value(10)
            panel.append(volume)
            items = Gtk.ListBox(selection_mode=Gtk.SelectionMode.SINGLE)
            for name in ('Alpha', 'Beta'):
                items.append(Gtk.Label(label=name))
            control_widgets['items'] = items
            items.select_row(items.get_row_at_index(0))
            panel.append(items)
            multiple = Gtk.ListBox(selection_mode=Gtk.SelectionMode.MULTIPLE)
            for name in ('Gamma', 'Delta'):
                multiple.append(Gtk.Label(label=name))
            multiple.select_all()
            panel.append(multiple)
            tabs = Gtk.Notebook()
            for name in ('First tab', 'Second tab'):
                tabs.append_page(Gtk.Label(label='Content for ' + name), Gtk.Label(label=name))
            panel.append(tabs)
            scroll = Gtk.ScrolledWindow(vexpand=True)
            scroll.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.ALWAYS)
            scroll.set_child(Gtk.Label(label='\n'.join('Line ' + str(i) for i in range(100))))
            panel.append(scroll)
            def recorded(*_):
                state_path = Path(sys.argv[1] + '.controls.json')
                temporary = state_path.with_suffix('.tmp')
                temporary.write_text(json.dumps({
                    'volume': volume.get_value(),
                    'completed': completed[0],
                    'selected': items.get_selected_row().get_index() if items.get_selected_row() else -1,
                    'multiselected': [row.get_index() for row in multiple.get_selected_rows()],
                    'tab': tabs.get_current_page(),
                    'scroll': scroll.get_vadjustment().get_value(),
                }))
                temporary.replace(state_path)
            volume.connect('value-changed', recorded)
            items.connect('row-selected', recorded)
            multiple.connect('selected-rows-changed', recorded)
            tabs.connect('switch-page', lambda *_: GLib.idle_add(lambda: (recorded(), False)[1]))
            scroll.get_vadjustment().connect('value-changed', recorded)
            controls.set_child(panel)
            extra_windows.append(controls)
            controls.present()
            recorded()
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
