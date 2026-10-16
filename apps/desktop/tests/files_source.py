#!/usr/bin/python3
"""Owned Nautilus instance on the host display, isolated from existing file windows."""
import os
from pathlib import Path
import subprocess
import sys
import gi
gi.require_version('Gio', '2.0')
from gi.repository import Gio

folder = Path(sys.argv[1]).resolve()
assert folder.is_dir() and os.environ.get('WAYLAND_DISPLAY')
root = folder.parent
session = Gio.bus_get_sync(Gio.BusType.SESSION, None)
address = session.call_sync('org.a11y.Bus', '/org/a11y/bus', 'org.a11y.Bus', 'GetAddress',
                            None, None, Gio.DBusCallFlags.NONE, 1000, None).unpack()[0]
config = root / 'source-bus.conf'
config.write_text('<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen><auth>EXTERNAL</auth><policy context="default"><allow user="*"/><allow own="*"/><allow send_destination="*"/><allow receive_sender="*"/></policy></busconfig>')
daemon = subprocess.Popen(['dbus-daemon', '--config-file=' + str(config), '--nofork', '--print-address=1'],
                          stdout=subprocess.PIPE, text=True)
app = None
try:
    private = daemon.stdout.readline().strip()
    if not private:
        raise RuntimeError('Private app bus unavailable')
    environment = dict(os.environ, GDK_BACKEND='wayland', GTK_A11Y='atspi',
                       AT_SPI_BUS_ADDRESS=address, DBUS_SESSION_BUS_ADDRESS=private,
                       GSETTINGS_BACKEND='memory', LANG='C.UTF-8', LC_ALL='C.UTF-8',
                       XDG_CONFIG_HOME=str(root / 'config'), XDG_DATA_HOME=str(root / 'data'),
                       XDG_CACHE_HOME=str(root / 'cache'))
    app = subprocess.Popen(['/usr/bin/nautilus', '--new-window', folder.as_uri()], env=environment)
    raise SystemExit(app.wait())
finally:
    if app and app.poll() is None:
        app.terminate()
        app.wait(timeout=5)
    daemon.terminate()
    daemon.wait(timeout=5)
