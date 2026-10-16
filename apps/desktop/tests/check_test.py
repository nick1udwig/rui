import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from apps.desktop.tests.check import run_check, test_environment, write_report


class AcceptanceRunnerTest(unittest.TestCase):
    def test_environment_removes_ambient_test_and_application_settings(self):
        self.assertEqual(test_environment({'PATH': '/bin', 'WAYLAND_DISPLAY': 'wayland-1',
            'RUI_WEBKIT_EXECUTABLE': '/webkit', 'RUI_CLIENT_ENGINE': 'webkit',
            'RUI_TEST_PROXY': '1', 'RUI_FILES_DIR': '/private'}),
            {'PATH': '/bin', 'WAYLAND_DISPLAY': 'wayland-1', 'RUI_WEBKIT_EXECUTABLE': '/webkit'})

    def test_timeout_retains_logs_and_kills_stubborn_process_group(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory)/'check.log'
            script = '''import os, signal, subprocess, sys, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
child = subprocess.Popen([sys.executable, '-c', 'import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(60)'])
print('ready', os.getpid(), child.pid, flush=True)
time.sleep(60)
'''
            start = time.monotonic()
            self.assertEqual(run_check([sys.executable, '-c', script], dict(os.environ), log,
                                       timeout=.5, grace=.1), 124)
            self.assertLess(time.monotonic() - start, 3)
            output = log.read_text()
            self.assertIn('runner deadline', output)
            _, parent, child = output.splitlines()[0].split()
            for pid in (parent, child):
                # An adopted zombie can briefly remain until init reaps it.
                status = Path('/proc')/pid/'stat'
                until = time.monotonic() + 2
                while status.exists() and status.read_text().rsplit(')', 1)[1].split()[0] != 'Z':
                    if time.monotonic() >= until:
                        self.fail(f'owned process {pid} survived timeout')
                    time.sleep(.01)

    def test_exit_codes_and_atomic_reports(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.assertEqual(run_check([sys.executable, '-c', 'print("diagnostic"); exit(7)'],
                                       dict(os.environ), root/'check.log'), 7)
            self.assertIn('diagnostic', (root/'check.log').read_text())
            path = root/'report.json'
            write_report(path, {'passed': True})
            write_report(path, {'passed': False, 'completed': False, 'checks': []})
            self.assertFalse(json.loads(path.read_text())['passed'])
            self.assertFalse(path.with_suffix('.tmp').exists())


if __name__ == '__main__':
    unittest.main()
