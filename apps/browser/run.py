#!/usr/bin/env python3
"""Build and launch the visible Chromium host and mobile web client."""
import argparse
import fcntl
import json
import os
import re
from pathlib import Path
import signal
import subprocess
import sys
import threading
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[2]


def run(*args):
    subprocess.run(args, cwd=ROOT, check=True)


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--host', default='localhost', help='hostname used by the phone')
    parser.add_argument('--listen', default='127.0.0.1')
    parser.add_argument('--port', type=int, default=8444, help='local listening port')
    parser.add_argument('--public-url', help='external HTTPS URL, optionally with a path prefix; enables loopback HTTP')
    parser.add_argument('--cert', type=Path, help='phone-trusted HTTPS certificate PEM')
    parser.add_argument('--key', type=Path, help='HTTPS private key PEM')
    parser.add_argument('--http-local', action='store_true', help='development only; loopback HTTP')
    parser.add_argument('--state-dir', type=Path, default=Path.home()/'.local/state/rui/browser')
    parser.add_argument('--files-dir', type=Path, default=Path.home()/'Downloads')
    parser.add_argument('--chromium', default='/usr/bin/chromium')
    parser.add_argument('--skip-build', action='store_true')
    parser.add_argument('--headless', action='store_true', help='automated testing only')
    args = parser.parse_args(argv)
    if not 1 <= args.port <= 65535 or any(c in args.host for c in '/?#@, \t\n:'):
        parser.error('invalid hostname or port')
    args.public_path = '/'
    if args.public_url:
        if args.http_local or args.cert or args.key or args.host != 'localhost':
            parser.error('--public-url cannot be combined with --http-local, --cert, --key, or --host')
        if args.listen not in ('127.0.0.1', '::1'):
            parser.error('--public-url requires a loopback listener behind the HTTPS proxy')
        try:
            public_url = args.public_url if '://' in args.public_url else 'https://' + args.public_url
            url = urlsplit(public_url)
            if (url.scheme != 'https' or not url.hostname or url.username is not None
                    or url.password is not None
                    or any(c in args.public_url for c in '?#\\')
                    or any(c.isspace() or ord(c) < 32 for c in args.public_url)
                    or len(url.hostname) > 253
                    or any(not re.fullmatch(r'[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?', label) for label in url.hostname.split('.'))
                    or url.netloc.endswith(':') or (url.port is not None and not 1 <= url.port <= 65535)):
                raise ValueError()
            path = url.path.rstrip('/')
            if url.path not in ('', '/') and (
                    len(url.path) > 1024 or '//' in url.path
                    or any(segment in ('', '.', '..') or not re.fullmatch(r'[a-zA-Z0-9._~-]+', segment)
                           for segment in path.split('/')[1:])):
                raise ValueError()
            args.public_path = path + '/'
            args.origin = f'https://{url.hostname}' + (f':{url.port}' if url.port not in (None, 443) else '')
        except ValueError:
            parser.error('--public-url must be an HTTPS URL such as https://foo.tailnet.ts.net/bart, with a plain path and no credentials, query, fragment, or dot segments')
    elif args.http_local:
        if args.host not in ('localhost', '127.0.0.1') or args.listen not in ('127.0.0.1', '::1') or args.cert or args.key:
            parser.error('--http-local requires a loopback host/listener and no HTTPS options')
    elif not args.cert or not args.key:
        parser.error('supply --cert and --key, --public-url for an HTTPS proxy, or --http-local for local development')
    if not args.public_url:
        scheme = 'http' if args.http_local else 'https'
        default_port = 80 if args.http_local else 443
        args.origin = f'{scheme}://{args.host}' + (f':{args.port}' if args.port != default_port else '')
    return args


def main():
    args = parse_args()
    if not args.files_dir.is_dir():
        raise SystemExit('--files-dir must be an existing directory')
    os.umask(0o077)
    directory = args.state_dir.expanduser().resolve()
    directory.mkdir(parents=True, exist_ok=True)
    if directory.stat().st_uid != os.getuid() or directory.stat().st_mode & 0o077:
        raise SystemExit('state directory must be owned by you with mode 0700')
    lock = (directory/'host.lock').open('w')
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        raise SystemExit('this browser state directory already has a running host')
    if not args.skip_build:
        run('cargo', 'build', '--locked', '-p', 'rui-browser-host')
        run(sys.executable, 'client-wasm/build.py')
        run('npm', '--prefix', 'client-browser', 'ci')
        run('npm', '--prefix', 'client-browser', 'run', 'build')
        run('npm', '--prefix', 'apps/browser', 'ci')
        run('npm', '--prefix', 'apps/browser', 'run', 'build')
    if not (directory/'host-key.der').exists():
        run('openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '365', '-subj', '/CN=rui-browser',
            '-addext', 'subjectAltName=DNS:rui-browser', '-addext', 'basicConstraints=critical,CA:FALSE',
            '-addext', 'keyUsage=critical,digitalSignature,keyEncipherment', '-addext', 'extendedKeyUsage=serverAuth',
            '-keyout', str(directory/'host-key.pem'), '-out', str(directory/'host.pem'))
        run('openssl', 'x509', '-in', str(directory/'host.pem'), '-outform', 'DER', '-out', str(directory/'host.der'))
        run('openssl', 'pkcs8', '-topk8', '-nocrypt', '-in', str(directory/'host-key.pem'), '-outform', 'DER', '-out', str(directory/'host-key.der'))
    (directory/'backend.json').unlink(missing_ok=True)
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--offline', '--locked', '--no-deps', '--format-version', '1'], cwd=ROOT))
    env = dict(os.environ, RUI_ORIGIN=args.origin, RUI_PUBLIC_PATH=args.public_path, RUI_WEB_PORT=str(args.port),
               RUI_WEB_LISTEN=args.listen, RUI_CHROMIUM=args.chromium, RUI_FILES_DIR=str(args.files_dir.resolve()),
               RUI_HEADLESS='1' if args.headless else '0')
    env.pop('RUI_WEB_CERT', None)
    env.pop('RUI_WEB_KEY', None)
    if args.cert:
        env.update(RUI_WEB_CERT=str(args.cert.resolve()), RUI_WEB_KEY=str(args.key.resolve()))
    process = subprocess.Popen([str(Path(metadata['target_directory'])/'debug/rui-browser-host'), str(directory), str(ROOT/'apps/browser/worker.mjs')], cwd=ROOT, env=env, start_new_session=True, stdin=subprocess.PIPE, text=True)
    def commands():
        try:
            for line in sys.stdin:
                process.stdin.write(line)
                process.stdin.flush()
        except (BrokenPipeError, ValueError):
            pass
    threading.Thread(target=commands, daemon=True).start()

    def stop(_signal=None, _frame=None):
        if process.poll() is None:
            try:
                process.stdin.write('quit\n')
                process.stdin.flush()
                process.wait(timeout=8)
            except (BrokenPipeError, subprocess.TimeoutExpired):
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        return process.wait()
    finally:
        stop()
        try:
            process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()


if __name__ == '__main__':
    raise SystemExit(main())
