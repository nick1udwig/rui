#!/usr/bin/env python3
"""Exercise the manual demo on owned Xvfb displays, never the user's desktop."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from fixtures.gtk.mirror_check import run_private


def wait(read, condition=bool, seconds=20):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            value = read()
            if condition(value):
                return value
        except (FileNotFoundError, json.JSONDecodeError):
            pass
        time.sleep(.05)
    raise AssertionError('demo state did not converge before the deadline')


def native(title, operation, *args):
    result = subprocess.run([sys.executable, str(ROOT / 'fixtures/render/native_events.py'), title, operation, *map(str, args)], capture_output=True, text=True, timeout=5)
    if result.returncode:
        raise AssertionError(result.stderr)


def newest(directory):
    paths = sorted(p for p in (directory / 'source').glob('whole-*.json') if p.name != 'whole-failure.json')
    return json.loads(paths[-1].read_text()) if paths else None


def has_label(frame, label):
    return any(n.get('label') == label for n in frame['semantics'])


def has_text(frame, text):
    return any(n.get('text', {}).get('content') == text for n in frame['semantics'])


def target(frame, label):
    node = next(n for n in frame['semantics'] if n.get('label') == label)
    surface = next(s for s in frame['surfaces'] if s['surface_key'] == 'main-content')
    binding = next(r for r in surface['binding_regions'] if r['semantic_id'] == node['id'])
    x, y, width, height = binding['interaction_region']
    return x + width / 2, y + height / 2


def run_case(target_dir, scenario):
    remote = scenario in ('remote', 'idle')
    with tempfile.TemporaryDirectory(prefix='rui-demo-check-') as tmp:
        directory, log = Path(tmp) / 'identity', Path(tmp) / 'demo.log'
        with log.open('w+') as output:
            args = [str(target_dir / 'rui-demo'), '--identity-dir', str(directory)]
            if remote:
                args += ['--host-only', '--listen', '0.0.0.0:0']
            host = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=output, stderr=subprocess.STDOUT, text=True)
            client = None
            try:
                frame = wait(lambda: newest(directory))
                assert has_label(frame, 'Host state: 0 clicks') and not frame['gaps']
                if scenario == 'idle':
                    time.sleep(2)
                    revision = newest(directory)['revision']
                    time.sleep(31)
                    assert host.poll() is None
                    assert newest(directory)['revision'] == revision, 'idle polling created revisions'
                    native('RUI Host', 'close')
                else:
                    if remote:
                        text = wait(log.read_text, lambda text: 'Visible GTK host listening at ' in text)
                        port = re.search(r'Visible GTK host listening at 0.0.0.0:(\d+)', text).group(1)
                        client = subprocess.Popen([str(target_dir / 'rui-mirror'), '127.0.0.1:' + port, 'rui-demo', str(directory / 'host.der'), str(directory / 'client.credential'), '03' * 16, '--control', '--scale=0.75'], stdout=output, stderr=subprocess.STDOUT)
                    title = 'RUI Mirror' if remote else 'RUI Client'
                    time.sleep(2)
                    if scenario == 'host-close':
                        native('RUI Host', 'close')
                    else:
                        bx, by = target(frame, 'Increment host counter')
                        native('RUI Host', 'click', bx, by)
                        frame = wait(lambda: newest(directory), lambda f: has_label(f, 'Host state: 1 clicks'))
                        time.sleep(1)
                        # Pinned GTK chrome: the canvas begins at Y=142. All
                        # source coordinates use the demo's 0.75 local scale.
                        native(title, 'click', bx * .75, by * .75 + 142)
                        frame = wait(lambda: newest(directory), lambda f: has_label(f, 'Host state: 2 clicks'))
                        time.sleep(.5)
                        ex, ey = target(frame, 'Editable text')
                        native(title, 'click', ex * .75, ey * .75 + 142)
                        time.sleep(.5)
                        native(title, 'keys', '+Control_L', 'a', '-Control_L')
                        time.sleep(.4)
                        native(title, 'keys', 'c', 'l', 'i', 'e', 'n', 't')
                        frame = wait(lambda: newest(directory), lambda f: has_text(f, 'client'))
                        native('RUI Host', 'click', ex, ey)
                        native('RUI Host', 'keys', '+Control_L', 'a', '-Control_L', 'h', 'o', 's', 't')
                        frame = wait(lambda: newest(directory), lambda f: has_text(f, 'host'))
                        if remote:
                            client.terminate()
                            client.wait(timeout=5)
                            assert host.poll() is None, 'disconnect stopped the remote host'
                            host.stdin.write('\n')
                            host.stdin.flush()
                        else:
                            native(title, 'close')
                host.wait(timeout=8)
                assert host.returncode == 0, log.read_text()
                assert not directory.exists(), 'demo identity was not cleaned up'
                print(json.dumps({'scenario': scenario, 'passed': True, 'final_source_revision': frame['revision']}), flush=True)
            except Exception:
                print(log.read_text(), file=sys.stderr)
                raise
            finally:
                for process in (client, host):
                    if process is not None and process.poll() is None:
                        process.terminate()
                        process.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--xvfb', type=Path, required=True)
    parser.add_argument('--target-dir', type=Path, default=ROOT / 'target/release')
    parser.add_argument('--scenario', choices=('local', 'remote', 'idle', 'host-close', 'all'), default='all')
    parser.add_argument('--child', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.child:
        run_case(args.target_dir.resolve(), args.scenario)
    else:
        scenarios = ('local', 'remote', 'idle', 'host-close') if args.scenario == 'all' else (args.scenario,)
        for scenario in scenarios:
            stdout = run_private(str(args.xvfb.resolve()), [sys.executable, __file__, '--child', '--xvfb', str(args.xvfb.resolve()), '--target-dir', str(args.target_dir.resolve()), '--scenario', scenario])
            print(stdout.strip(), flush=True)


if __name__ == '__main__':
    main()
