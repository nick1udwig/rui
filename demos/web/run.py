#!/usr/bin/env python3
"""Launch the visible GTK host and Vite browser playground together."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]


def interrupted(_signal, _frame):
    # Route termination through the same owned-child/temp-directory cleanup
    # as Ctrl+C; Vite and the host run in separate process groups.
    raise KeyboardInterrupt


def run(*command):
    try:
        subprocess.run(command, cwd=ROOT, check=True)
    except subprocess.CalledProcessError as error:
        # The failed tool already printed its diagnostic; keep its exit status
        # without hiding the useful message under a launcher traceback.
        raise SystemExit(error.returncode) from None


def stop(process, graceful=False):
    if process is None:
        return
    if graceful and process.poll() is None:
        try:
            process.stdin.write('\n')
            process.stdin.flush()
            process.wait(timeout=8)
        except (BrokenPipeError, OSError, subprocess.TimeoutExpired):
            pass
    # Include the launcher-owned Vite/Python descendants, even if npm exited.
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=3)


def main():
    signal.signal(signal.SIGTERM, interrupted)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--presentation', choices=('mirror','semantic'), default='mirror')
    parser.add_argument('--application', choices=('corpus','editor'), default='corpus')
    parser.add_argument('--public-host', default='localhost', help='hostname or IP used in the browser URL')
    parser.add_argument('--listen', default='127.0.0.1:8443', help='Rust host bind address; Vite binds 0.0.0.0:5173')
    parser.add_argument('--cert', type=Path, help='browser-trusted HTTPS certificate')
    parser.add_argument('--key', type=Path, help='matching HTTPS private key')
    parser.add_argument('--wasm-bindgen', default=os.environ.get('WASM_BINDGEN'))
    parser.add_argument('--skip-build', action='store_true', help='reuse previously built modules and installed packages')
    args = parser.parse_args()
    try:
        port_number = int(args.listen.rsplit(":", 1)[1])
        if not 1 <= port_number <= 65535:
            raise ValueError()
    except (IndexError, ValueError):
        parser.error("--listen requires a fixed port between 1 and 65535")
    if bool(args.cert) != bool(args.key):
        parser.error('supply both --cert and --key')
    if any(c in args.public_host for c in '/?#@, \t\n'):
        parser.error('--public-host must be a hostname or IP, without a URL scheme or port')
    demo = 'demos/semantic' if args.presentation == 'semantic' else 'demos/web'
    if not args.skip_build:
        run('cargo', 'build', '--release', '--locked', '-p', 'rui-client-desktop', '--features', 'gtk-demo', '--bin', 'rui-demo')
        run('python3', 'client-wasm/build.py', *(['--wasm-bindgen', args.wasm_bindgen] if args.wasm_bindgen else []))
        run('npm', '--prefix', 'client-browser', 'ci')
        run('npm', '--prefix', 'client-browser', 'run', 'build')
        run('npm', '--prefix', demo, 'ci')
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--offline', '--no-deps', '--format-version', '1'], cwd=ROOT))
    binary = Path(metadata['target_directory']) / 'release/rui-demo'
    scheme = 'https' if args.cert else 'http'
    origin = f'{scheme}://{args.public_host}:5173'
    host = web = None
    with tempfile.TemporaryDirectory(prefix='rui-browser-demo-') as temporary:
        identity = Path(temporary) / 'identity'
        try:
            origins = sorted({origin, f'{scheme}://localhost:5173', f'{scheme}://127.0.0.1:5173'})
            arguments = [str(binary), '--host-only', '--application', args.application, '--listen', args.listen, '--identity-dir', str(identity)]
            for allowed in origins:
                arguments += ['--browser-origin', allowed]
            host = subprocess.Popen(arguments, cwd=ROOT, stdin=subprocess.PIPE, text=True, start_new_session=True)
            deadline = time.monotonic() + 20
            while not (identity / 'client.credential').exists():
                if host.poll() is not None:
                    raise RuntimeError('GTK host stopped during startup')
                if time.monotonic() > deadline:
                    raise RuntimeError('GTK host startup timed out')
                time.sleep(.05)
            _, port = args.listen.rsplit(':', 1)
            address = args.listen.rsplit(':', 1)[0]
            if address in ('0.0.0.0', '::', '[::]'):
                address = '127.0.0.1' if address == '0.0.0.0' else '[::1]'
            environment = dict(os.environ, RUI_DEMO_IDENTITY=str(identity), RUI_HOST_URL=f'wss://{address}:{port}', RUI_WEB_HOSTS=args.public_host)
            if args.cert:
                environment.update(RUI_WEB_CERT=str(args.cert.resolve()), RUI_WEB_KEY=str(args.key.resolve()))
            web = subprocess.Popen(['npm', '--prefix', demo, 'run', 'dev'], cwd=ROOT, env=environment, start_new_session=True)
            print(f'\nOpen {origin} and choose {identity / "client.credential"}.', flush=True)
            print('Observe the semantic view.' if args.presentation == 'semantic' else 'Take control, then click a button or edit a text field.', flush=True)
            print('Close the host or press Ctrl+C to stop.', flush=True)
            while host.poll() is None and web.poll() is None:
                time.sleep(.1)
            if host.poll() not in (None, 0) or web.poll() not in (None, 0):
                raise RuntimeError('host or browser server stopped with an error')
        except KeyboardInterrupt:
            pass
        finally:
            stop(web)
            stop(host, graceful=True)


if __name__ == '__main__':
    main()
