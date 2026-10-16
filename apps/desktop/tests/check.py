#!/usr/bin/env python3
"""Run the semantic desktop checks and retain a machine-readable local report."""
import argparse
from datetime import datetime, timezone
import json
import os
import signal
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[3]


def test_environment(environment):
    # Preserve the explicit WebKit installation override, not application or
    # individual-test flags that can silently change acceptance coverage.
    return {key: value for key, value in environment.items()
            if not key.startswith('RUI_') or key == 'RUI_WEBKIT_EXECUTABLE'}


def write_report(path, report):
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(report, indent=2) + '\n')
    temporary.replace(path)


def run_check(command, environment, log_path, timeout=180, grace=3):
    # Stream to disk: partial output survives interruption and descendants
    # cannot hold a capture pipe open after the direct child exits.
    with log_path.open('w') as log:
        process = subprocess.Popen(command, cwd=ROOT, env=environment,
                                   stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        def signal_group(sig):
            try:
                os.killpg(process.pid, sig)
            except ProcessLookupError:
                pass
        try:
            try:
                return process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                log.write(f'\nCheck exceeded the {timeout} second runner deadline.\n')
                return 124
        finally:
            signal_group(signal.SIGTERM)
            try:
                process.wait(timeout=grace)
            except subprocess.TimeoutExpired:
                pass
            finally:
                signal_group(signal.SIGKILL)
                process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--xvfb', required=True, type=Path)
    parser.add_argument('--real-desktop', action='store_true', help='also open owned temporary apps on the existing Wayland desktop')
    parser.add_argument('--engine', action='append', choices=('chromium', 'webkit'))
    parser.add_argument('--output', type=Path, default=ROOT / 'apps/desktop/test-output/checks')
    args = parser.parse_args()
    if not args.xvfb.is_file():
        parser.error('--xvfb must name an installed Xvfb binary')
    if args.real_desktop and not os.environ.get('WAYLAND_DISPLAY'):
        parser.error('--real-desktop requires WAYLAND_DISPLAY from the host desktop session')
    args.output.mkdir(parents=True, exist_ok=True)
    report = {'started': datetime.now(timezone.utc).isoformat(), 'physical_device': False,
              'completed': False, 'passed': False, 'checks': []}
    report_path = args.output / 'report.json'
    write_report(report_path, report)
    base = test_environment(os.environ)
    checks = [
        ('runner', [sys.executable, '-m', 'unittest', 'apps.desktop.tests.check_test'], {}),
        ('protocol-streams', ['node', '--test', 'apps/browser/tests/json-stream.test.mjs', 'apps/browser/tests/framed-rpc.test.mjs'], {}),
        ('semantic-identities', ['node', '--test', 'apps/shared/tests/semantic-nodes.test.mjs'], {}),
        ('desktop-trees', ['node', '--test', 'apps/desktop/tests/tree.test.mjs'], {}),
        ('native-dispatch', [sys.executable, '-m', 'unittest', 'apps.desktop.tests.native_test'], {}),
        ('workspace', ['node', 'apps/desktop/tests/workspace.test.mjs'], {}),
        ('browser-bridge', ['node', '--test', 'apps/desktop/tests/browser.test.mjs'], {}),
        ('browser-workspace', ['node', '--test', 'apps/browser/tests/workspace.test.mjs'], {}),
        ('atspi-regression', [sys.executable, '-m', 'unittest', 'adapters.atspi.test_observer', 'adapters.atspi.test_view'], {}),
        ('launcher', [sys.executable, '-m', 'unittest', 'discover', '-s', 'apps/browser/tests', '-p', '*_test.py'], {}),
        ('host-tests', ['cargo', 'test', '--offline', '--locked', '-p', 'rui-browser-host'], {}),
        ('clippy', ['cargo', 'clippy', '--offline', '--locked', '-p', 'rui-browser-host', '--all-targets', '--', '-D', 'warnings'], {}),
        ('host-build', ['cargo', 'build', '--offline', '--locked', '-p', 'rui-browser-host'], {}),
        ('client-build', ['npm', '--prefix', 'apps/browser', 'run', 'build'], {}),
        ('patch-format', ['git', 'diff', '--check'], {}),
    ]
    native = [sys.executable, 'fixtures/atspi/native_check.py', '--xvfb', str(args.xvfb.resolve()),
              '--driver', 'apps/desktop/tests/native_driver.py', '--output']
    checks.append(('native', native + [str(args.output / 'native')], {}))
    for engine in args.engine or ['chromium', 'webkit']:
        checks.append((f'mobile-{engine}', native + [str(args.output / engine)],
                       {'RUI_DESKTOP_LIVE': '1', 'RUI_CLIENT_ENGINE': engine}))
        checks.append((f'desktop-browser-{engine}', ['node', 'apps/browser/tests/live.mjs'],
                       {'RUI_TEST_DESKTOP_BROWSER': '1', 'RUI_TEST_PROXY': '1', 'RUI_TEST_PATH': '/desktop',
                        'RUI_CLIENT_ENGINE': engine, **({'RUI_CLIENT_EXECUTABLE': base['RUI_WEBKIT_EXECUTABLE']} if engine == 'webkit' and base.get('RUI_WEBKIT_EXECUTABLE') else {})}))
        if args.real_desktop:
            for app in ('writer', 'files'):
                checks.append((f'{app}-{engine}', ['node', 'apps/desktop/tests/existing-desktop.mjs'],
                               {'RUI_CLIENT_ENGINE': engine, 'RUI_REAL_APP': app}))
    checks.append(('browser-regression', ['node', 'apps/browser/tests/live.mjs'], {}))
    for name, command, environment in checks:
        print(f'Running {name}…', flush=True)
        started = time.monotonic()
        log_path = args.output / f'{name}.log'
        code = run_check(command, base | environment, log_path)
        report['checks'].append({'name': name, 'passed': code == 0, 'seconds': round(time.monotonic() - started, 2)})
        report['completed'] = len(report['checks']) == len(checks)
        report['passed'] = report['completed'] and all(check['passed'] for check in report['checks'])
        write_report(report_path, report)
        print(f'{name}: {"passed" if code == 0 else "FAILED"}', flush=True)
        if code:
            print(log_path.read_text(errors='replace')[-6000:], file=sys.stderr)
            return 1
    print(f'All {len(checks)} checks passed. Report: {args.output / "report.json"}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
