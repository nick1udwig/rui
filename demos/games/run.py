#!/usr/bin/env python3
"""Run a native MAME game and its RUI browser mirror."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]


def run(*args):
    subprocess.run(args, cwd=ROOT, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--game', choices=['asteroid'], default='asteroid')
    parser.add_argument('--mame', default='mame')
    parser.add_argument('--rompath', type=Path, required=True)
    parser.add_argument('--plugins', type=Path)
    parser.add_argument('--public-host', default='localhost')
    parser.add_argument('--cert', type=Path)
    parser.add_argument('--key', type=Path)
    parser.add_argument('--skip-build', action='store_true')
    parser.add_argument('--state', type=Path, help='keep logs and credentials in this new directory')
    args = parser.parse_args()
    if bool(args.cert) != bool(args.key):
        parser.error('supply both --cert and --key')
    if any(c in args.public_host for c in '/?#@, \t\n'):
        parser.error('public host must be a hostname or IP')
    run(args.mame, args.game, '-rompath', str(args.rompath.resolve()), '-verifyroms')
    if not args.skip_build:
        run('cargo', 'build', '--release', '--locked', '-p', 'rui-browser-host')
        run('python3', 'client-wasm/build.py')
        run('npm', '--prefix', 'client-browser', 'ci')
        run('npm', '--prefix', 'client-browser', 'run', 'build')
        run('npm', '--prefix', 'demos/games', 'ci')
        run('npm', '--prefix', 'demos/games', 'run', 'build')
    temporary = None
    if args.state:
        directory = args.state.resolve()
        directory.mkdir(mode=0o700)
    else:
        temporary = tempfile.TemporaryDirectory(prefix='rui-game-')
        directory = Path(temporary.name)
    child = None
    try:
        run('openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '2', '-subj', '/CN=rui-demo', '-addext', 'subjectAltName=DNS:rui-demo', '-keyout', str(directory/'host-key.pem'), '-out', str(directory/'host.pem'))
        run('openssl', 'x509', '-in', str(directory/'host.pem'), '-outform', 'DER', '-out', str(directory/'host.der'))
        run('openssl', 'pkcs8', '-topk8', '-nocrypt', '-in', str(directory/'host-key.pem'), '-outform', 'DER', '-out', str(directory/'host-key.der'))
        metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--offline', '--no-deps', '--format-version', '1'], cwd=ROOT))
        environment = dict(os.environ, RUI_WORKSPACE='game', RUI_GAME=args.game, RUI_MAME=args.mame, RUI_ROMPATH=str(args.rompath.resolve()), RUI_STATE=str(directory), RUI_ORIGIN=f'{"https" if args.cert else "http"}://{args.public_host}:5173', RUI_WEB_HOSTS=args.public_host)
        if args.plugins:
            environment['RUI_MAME_PLUGINS'] = str(args.plugins.resolve())
        if args.cert:
            environment.update(RUI_WEB_CERT=str(args.cert.resolve()), RUI_WEB_KEY=str(args.key.resolve()))
        child = subprocess.Popen([str(Path(metadata['target_directory'])/'release/rui-browser-host'), str(directory), str(ROOT/'demos/games/worker.mjs')], cwd=ROOT, env=environment, stdin=subprocess.PIPE, start_new_session=True, text=True)
        def interrupted(*_):
            raise KeyboardInterrupt
        signal.signal(signal.SIGTERM, interrupted)
        signal.signal(signal.SIGINT, interrupted)
        return child.wait()
    except KeyboardInterrupt:
        return 0
    finally:
        if child:
            try:
                os.killpg(child.pid, signal.SIGTERM)
                child.wait(timeout=5)
            except (ProcessLookupError, subprocess.TimeoutExpired):
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait()
        if temporary:
            temporary.cleanup()


if __name__ == '__main__':
    raise SystemExit(main())
