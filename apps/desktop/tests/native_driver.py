#!/usr/bin/python3
"""Proof on an owned display, using a source running before the adapter starts."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from apps.desktop.native import DesktopReader


def main():
    config = tempfile.NamedTemporaryFile(mode='w', suffix='.conf')
    config.write('<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen><auth>EXTERNAL</auth><policy context="default"><allow user="*"/><allow own="*"/><allow send_destination="*"/><allow receive_sender="*"/></policy></busconfig>')
    config.flush()
    daemon = subprocess.Popen(['dbus-daemon', '--config-file=' + config.name, '--nofork', '--print-address=1'], stdout=subprocess.PIPE, text=True)
    address = daemon.stdout.readline().strip()
    os.environ['DBUS_SESSION_BUS_ADDRESS'] = address
    os.environ['AT_SPI_BUS_ADDRESS'] = address
    registry = subprocess.Popen(['/usr/lib/at-spi2-registryd'], stdout=subprocess.DEVNULL)
    try:
        time.sleep(.15)
        if os.environ.get('RUI_DESKTOP_LIVE') == '1':
            subprocess.run(['node', str(Path(__file__).with_name('live.mjs'))], check=True, timeout=50)
        else:
            run()
    finally:
        registry.terminate(); registry.wait(timeout=3)
        daemon.terminate(); daemon.wait(timeout=3)
        config.close()


def run():
    assert os.environ.get('RUI_ISOLATED_DISPLAY') == '1'
    with tempfile.TemporaryDirectory() as directory:
        target = Path(directory) / 'document.txt'
        source = subprocess.Popen(['/usr/bin/python3', str(Path(__file__).with_name('source.py')), str(target)],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        try:
            assert json.loads(source.stdout.readline())['ready']
            reader = DesktopReader()
            app = next(a for a in reader.discover()['apps'] if a['pid'] == source.pid)
            capture = DesktopReader().sample(app)
            editor = next(n for n in capture['nodes'] if n.get('label') == 'Document')
            assert editor['value'] == 'Already open on the host', editor
            assert 'SetValue' in editor['actions'], editor
            assert 'never-publish' not in json.dumps(capture)
            assert any(n['label'] == 'Protected field — unavailable' and not n['actions'] for n in capture['nodes'])
            assert any(n['label'] == 'Password field — unavailable' and not n['actions'] for n in capture['nodes'])
            def dispatch(capture, node, action, value=None):
                return DesktopReader().execute({'op': 'dispatch', 'source': app, 'key': node['key'],
                    'fingerprint': node['fingerprint'], 'action': action, 'value': value, 'expires': time.time() * 1000 + 2000})
            assert dispatch(capture, editor, 'SetValue', 'Phone edit — 東京 👋')['status'] == 'dispatched'
            assert dispatch(capture, editor, 'SetValue', 'stale overwrite')['status'] == 'rejected'
            # An unrelated document/status change must not invalidate Save.
            save = next(n for n in capture['nodes'] if n.get('label') == 'Save document')
            assert dispatch(capture, save, 'Activate')['status'] == 'dispatched'
            for _ in range(100):
                if target.exists(): break
                time.sleep(.01)
            assert target.read_text() == 'Phone edit — 東京 👋'
            source.stdin.write('{"op":"modal"}\n'); source.stdin.flush(); time.sleep(.2)
            modal = DesktopReader().sample(app)
            assert not next(n for n in modal['nodes'] if n.get('label') == 'Save document')['actions']
            dismiss = next(n for n in modal['nodes'] if n.get('label') == 'Dismiss dialog')
            assert dispatch(modal, dismiss, 'Activate')['status'] == 'dispatched'
            os.kill(source.pid, signal.SIGSTOP)
            before = time.monotonic()
            try:
                stalled = subprocess.run(['/usr/bin/python3', str(ROOT/'apps/desktop/native.py')],
                    input=json.dumps({'op': 'capture', 'source': app}), capture_output=True, text=True, timeout=3, check=True)
                assert 'error' in json.loads(stalled.stdout)
            finally:
                os.kill(source.pid, signal.SIGCONT)
            assert time.monotonic() - before < 3
            source.terminate(); source.wait(timeout=3)
            try:
                DesktopReader().sample(app)
                raise AssertionError('exited application accepted')
            except ValueError:
                pass
            print(json.dumps({'native': 'passed', 'existing_app': True, 'unicode_edit_and_save': True,
                              'password_redacted': True, 'stale_rejected': True, 'modal_guard': True,
                              'stall_bounded': True, 'exit_rejected': True}))
        finally:
            if source.poll() is None:
                source.kill(); source.wait(timeout=3)


if __name__ == '__main__':
    main()
