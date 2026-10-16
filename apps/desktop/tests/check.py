#!/usr/bin/env python3
"""Run the semantic desktop checks and retain a machine-readable local report."""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[3]


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
    report = {'started': datetime.now(timezone.utc).isoformat(), 'physical_device': False, 'checks': []}
    base = dict(os.environ)
    base.pop('RUI_DESKTOP_LIVE', None)
    base.pop('RUI_CLIENT_ENGINE', None)
    checks = [
        ('workspace', ['node', 'apps/desktop/tests/workspace.test.mjs'], {}),
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
        if args.real_desktop:
            for app in ('writer', 'files'):
                checks.append((f'{app}-{engine}', ['node', 'apps/desktop/tests/existing-desktop.mjs'],
                               {'RUI_CLIENT_ENGINE': engine, 'RUI_REAL_APP': app}))
    checks.append(('browser-regression', ['node', 'apps/browser/tests/live.mjs'], {}))
    for name, command, environment in checks:
        print(f'Running {name}…', flush=True)
        started = time.monotonic()
        try:
            result = subprocess.run(command, cwd=ROOT, env=base | environment, capture_output=True, text=True, timeout=180)
            code, log = result.returncode, result.stdout + result.stderr
        except subprocess.TimeoutExpired:
            code, log = 124, 'Check exceeded the 180 second runner deadline.'
        (args.output / f'{name}.log').write_text(log)
        report['checks'].append({'name': name, 'passed': code == 0, 'seconds': round(time.monotonic() - started, 2)})
        report['passed'] = all(check['passed'] for check in report['checks'])
        (args.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(f'{name}: {"passed" if code == 0 else "FAILED"}', flush=True)
        if code:
            print(log[-6000:], file=sys.stderr)
            return 1
    print(f'All {len(checks)} checks passed. Report: {args.output / "report.json"}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
