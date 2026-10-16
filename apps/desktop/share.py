#!/usr/bin/env python3
"""Local GTK sharing picker; communicates only over the owner's private socket."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import socket
import sys


def request(directory, command, app_id=None):
    path = Path(directory).expanduser().resolve()
    info = path.stat()
    if info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise ValueError('Host state directory must belong to you with mode 0700')
    message = {'op': 'desktop-protect'} if command == 'protect' else {
        'op': 'desktop', 'command': command, 'id': app_id,
    }
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(6)
        connection.connect(str(path / 'admin.sock'))
        connection.sendall(json.dumps(message).encode() + b'\n')
        data = bytearray()
        while b'\n' not in data:
            part = connection.recv(8192)
            if not part:
                raise ConnectionError('Host disconnected')
            data.extend(part)
            if len(data) > 65536:
                raise ValueError('Host reply exceeds limit')
    result = json.loads(data)
    if 'error' in result:
        raise ValueError(result['error'])
    return result


def run_picker(directory):
    # Register this kernel-observed PID before GTK can publish any controls.
    # The picker remains accessible locally, but RUI cannot share its authority.
    request(directory, 'protect')
    # Preserve exclusion across a host restart without trusting a UI label.
    Path('/proc/self/comm').write_text('rui-share-ui')
    import gi
    gi.require_version('Gtk', '4.0')
    from gi.repository import Gtk, GLib, Gio
    GLib.set_application_name('RUI Desktop Sharing')
    app = Gtk.Application(application_id='org.rui.DesktopSharing',
                          flags=Gio.ApplicationFlags.NON_UNIQUE)
    pool = ThreadPoolExecutor(max_workers=1)
    closed = False
    reports = {}

    def activate(application):
        window = Gtk.ApplicationWindow(application=application, title='RUI Desktop Sharing',
                                       default_width=620, default_height=520)
        layout = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12,
                         margin_top=18, margin_bottom=18, margin_start=18, margin_end=18)
        intro = Gtk.Label(label='Share existing applications with paired devices', xalign=0)
        intro.add_css_class('title-2')
        layout.append(intro)
        layout.append(Gtk.Label(label='Sharing includes accessible windows and dialogs. Approvals end when the host exits.',
                                wrap=True, xalign=0))
        refresh = Gtk.Button(label='Refresh applications')
        layout.append(refresh)
        status = Gtk.Label(label='', wrap=True, xalign=0, selectable=True)
        layout.append(status)
        rows = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        scroll = Gtk.ScrolledWindow(vexpand=True)
        scroll.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        scroll.set_child(rows)
        layout.append(scroll)
        window.set_child(layout)

        def submit(command, app_id=None):
            rows.set_sensitive(False)
            refresh.set_sensitive(False)
            status.set_label('Checking application support…' if command == 'diagnose' else 'Updating sharing…')

            def work():
                result = request(directory, command, app_id)
                listing = result if command == 'apps' else request(directory, 'apps')
                return result, listing

            def finish(future):
                def update():
                    if closed:
                        return False
                    try:
                        result, listing = future.result()
                        if command == 'diagnose':
                            reports[app_id] = result
                        render(listing['apps'])
                        status.set_label('Support checked; this does not share the application.' if command == 'diagnose'
                                         else 'Sharing updated.' if command != 'apps'
                                         else 'Choose an application below. Apps without accessibility may be absent.')
                    except Exception as error:
                        status.set_label(f'Unable to update: {error}. Start the desktop host, then refresh.')
                    finally:
                        rows.set_sensitive(True)
                        refresh.set_sensitive(True)
                    return False
                GLib.idle_add(update)
            pool.submit(work).add_done_callback(finish)

        def render(apps):
            while rows.get_first_child():
                rows.remove(rows.get_first_child())
            if not apps:
                rows.append(Gtk.Label(label='No accessible applications found.', xalign=0))
            for source in apps:
                identity = f"{source['name']} ({source['pid']})"
                row = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
                row.append(Gtk.Label(label=identity, xalign=0, wrap=True))
                row.append(Gtk.Label(label='Shared' if source['shared'] else 'Not shared', xalign=0))
                actions = Gtk.Box(spacing=8)
                change = Gtk.Button(label='Stop sharing' if source['shared'] else 'Share')
                change.set_sensitive(source['shared'] or source.get('available', True))
                change.connect('clicked', lambda _, entry=source: submit('unshare' if entry['shared'] else 'share', entry['id']))
                actions.append(change)
                check = Gtk.Button(label='Check support')
                check.set_sensitive(source.get('available', True))
                check.connect('clicked', lambda _, app_id=source['id']: submit('diagnose', app_id))
                actions.append(check)
                row.append(actions)
                if not source.get('available', True):
                    row.append(Gtk.Label(label='Application unavailable or closed. Stop sharing to remove it.', wrap=True, xalign=0))
                report = reports.get(source['id'])
                if report:
                    counts = report['counts']
                    summary = (f"{counts['windows']} windows · {counts['editable_text']} editable text fields · "
                               f"{counts['numeric_controls']} numeric controls · {counts['selection_controls']} selection controls · "
                               f"{counts['actions']} actions · {counts['protected_fields']} protected fields")
                    row.append(Gtk.Label(label=summary + '\n' + report['scope'], wrap=True, xalign=0))
                rows.append(row)
        refresh.connect('clicked', lambda _: submit('apps'))
        window.present()
        submit('apps')

    app.connect('activate', activate)
    try:
        return app.run([])
    finally:
        closed = True
        pool.shutdown(wait=False, cancel_futures=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--state-dir', type=Path, default=Path.home() / '.local/state/rui/desktop')
    parser.add_argument('--list', action='store_true', help='print local application metadata without opening GTK')
    parser.add_argument('--diagnose', metavar='APP_ID', help='print local capability counts; does not share the app')
    args = parser.parse_args()
    try:
        if args.list or args.diagnose:
            print(json.dumps(request(args.state_dir, 'diagnose' if args.diagnose else 'apps', args.diagnose), indent=2))
            return 0
        return run_picker(args.state_dir)
    except Exception as error:
        print(f'RUI Desktop Sharing: {error}. Start apps/desktop/run.py first.', file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
