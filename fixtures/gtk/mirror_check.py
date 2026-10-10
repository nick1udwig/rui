#!/usr/bin/env python3
"""Real GTK producer/mirror proof on a private authenticated Xvfb/session bus."""
import argparse
import hashlib
import json
import os
import signal
from pathlib import Path
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from fixtures.gtk.compare import CRITERIA, bounded_read, native_glyph_geometry, pixels
from fixtures.gtk.composition_trace import verify_controller


def run_private(xvfb, command, *, timeout=45):
    """Owned displays and service-free private bus; deadline kills all children."""
    with tempfile.TemporaryDirectory(prefix='rui-gtk-mirror-display-') as runtime:
        displays = [':' + str(25000 + os.getpid() % 19000), ':' + str(45000 + os.getpid() % 19000)]
        authorities = [runtime + '/mirror-authority', runtime + '/source-authority']
        for display, authority in zip(displays, authorities):
            subprocess.run(['xauth', '-f', authority, 'add', display, 'MIT-MAGIC-COOKIE-1', os.urandom(16).hex()], check=True, capture_output=True)
        config = Path(runtime) / 'bus.conf'
        config.write_text(f'<busconfig><type>session</type><listen>unix:tmpdir={runtime}</listen><auth>EXTERNAL</auth><policy context="default"><allow user="{os.getuid()}"/><allow own="*"/><allow send_destination="*"/><allow receive_sender="*"/></policy></busconfig>')
        env = os.environ.copy()
        env.update(XDG_RUNTIME_DIR=runtime, GSK_RENDERER='cairo', GTK_IM_MODULE='simple', GTK_A11Y='none', GSETTINGS_BACKEND='memory', GDK_BACKEND='x11', DISPLAY=displays[0], XAUTHORITY=authorities[0], GIO_USE_VFS='local', GTK_USE_PORTAL='0', RUI_ISOLATED_DISPLAY='1', RUI_GTK_SOURCE_DISPLAY=displays[1], RUI_GTK_SOURCE_XAUTHORITY=authorities[1])
        env.pop('WAYLAND_DISPLAY', None)
        servers = []
        child = None
        with open(runtime + '/display.log', 'w+') as log:
            try:
                for index, (display, authority) in enumerate(zip(displays, authorities)):
                    servers.append(subprocess.Popen([xvfb, display, '-screen', '0', '2300x1100x24' if index == 0 else '1280x1024x24', '-nolisten', 'tcp', '-auth', authority, '-noreset'], env=env, stdout=log, stderr=subprocess.STDOUT))
                time.sleep(.2)
                if any(server.poll() is not None for server in servers):
                    log.seek(0)
                    raise RuntimeError(log.read(8192))
                child = subprocess.Popen(['dbus-run-session', '--config-file=' + str(config), '--', *command], env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
                try:
                    stdout, stderr = child.communicate(timeout=timeout)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGTERM)
                    try:
                        stdout, stderr = child.communicate(timeout=2)
                    except subprocess.TimeoutExpired:
                        os.killpg(child.pid, signal.SIGKILL)
                        stdout, stderr = child.communicate(timeout=2)
                    raise RuntimeError('private mirror deadline: ' + stdout[-4096:] + stderr[-8192:])
                if child.returncode or 'Traceback' in stderr:
                    raise RuntimeError(stdout[-4096:] + stderr[-8192:])
            finally:
                # The wrapper may exit while a producer descendant survives.
                # Always dispose this exact owned process group, including on
                # early wrapper failure; no unrelated desktop process is used.
                if child is not None:
                    for action in (signal.SIGTERM, signal.SIGKILL):
                        try:
                            os.killpg(child.pid, action)
                        except ProcessLookupError:
                            pass
                        try:
                            child.wait(timeout=1)
                        except subprocess.TimeoutExpired:
                            pass
                for server in servers:
                    server.terminate()
                for server in servers:
                    server.wait(timeout=5)
    return stdout


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--xvfb', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--probe', type=Path, default=Path('target/release/examples/gtk_source_probe'))
    parser.add_argument('--scenario', choices=('mirror', 'composition'), default='mirror')
    args = parser.parse_args()
    if args.output.exists() and any(args.output.iterdir()):
        raise ValueError('fresh owned output directory required')
    stdout = run_private(args.xvfb, [str(args.probe.resolve()), str(args.output), args.scenario])
    report_path = args.output / 'interaction.json'
    report = json.loads(bounded_read(report_path, 65536))
    if args.scenario == 'composition':
        report['wire_composition_trace'] = verify_controller(args.output / 'controller.trace.jsonl', clock_policy=report.get('clock_policy', 'native-gtk'))
    vector = args.output / 'source-vector'
    references = []
    for path in sorted(vector.glob('whole-*.json')):
        manifest = json.loads(bounded_read(path, 1 << 20))
        comparison = {'capture': path.name, 'envelope_sha256': manifest['envelope_sha256'],
                      'local_reference_sha256': manifest['local_reference']['sha256'],
                      'geometry': native_glyph_geometry(manifest, vector),
                      'pixels': pixels(manifest, vector, path.with_suffix('.pam'))}
        if not comparison['pixels']['passed']:
            raise ValueError('actual native source reference differs beyond declared backend tolerance')
        references.append(comparison)
    if len(references) != (5 if args.scenario == 'composition' else 2) or report['host_memory_after_disposal'] != 0:
        raise ValueError('missing native initial/resulting capture or leaked owner memory')
    report['native_reference_comparison'] = {'criteria': CRITERIA, 'captures': references,
        'scope': 'Exact native same-node local diagnostic composition versus verified source projection; actual after-paint mirror texture digest equals this projection over opaque white.'}
    report['retained_exact_trace_sha256'] = {
        str(path.relative_to(args.output)): hashlib.sha256(bounded_read(path, 4 << 20)).hexdigest()
        for path in sorted(args.output.rglob('*'))
        if path.is_file() and (path.parent == vector and path.suffix in ('.bin', '.json', '.rvp') or path.parent == args.output and path.name.endswith('.trace.jsonl'))
    }
    # Bind the evidence to the exact accepted implementation used to build
    # this probe. Native reference images stay local and outside this list.
    implementation = [
        'client-core/src/replica.rs', 'client-core/src/session.rs',
        'client-core/src/command_queue.rs', 'client-core/src/resource_requests.rs',
        'wire/src/codec.rs', 'adapters/sdk/src/publication_builder.rs',
        'Cargo.lock', 'adapters/sdk/Cargo.toml', 'adapters/sdk/producer.py',
        'adapters/sdk/src/lib.rs', 'adapters/sdk/src/vector_audit.rs',
        'adapters/sdk/src/gtk_host.rs', 'adapters/sdk/src/gtk_port.rs',
        'adapters/sdk/src/native_fence.rs', 'adapters/sdk/src/source_identity.rs',
        'adapters/gtk4/capture.py', 'adapters/gtk4/state.py',
        'adapters/gtk4/native.py', 'adapters/gtk4/native_fence.py',
        'adapters/gtk4/transaction.py',
        'fixtures/gtk/corpus.py', 'fixtures/gtk/whole_view.py',
        'fixtures/gtk/whole_native_source.py', 'fixtures/gtk/compare.py',
        'fixtures/gtk/mirror_check.py', 'fixtures/gtk/place_windows.py',
        'fixtures/gtk/composition_trace.py',
        'fixtures/render/native_events.py', 'client-desktop/Cargo.toml',
        'client-desktop/examples/gtk_source_probe.rs',
        'client-desktop/examples/corpus_support/trace.rs',
        'client-desktop/src/desktop.rs', 'client-desktop/src/desktop/gtk.rs',
        'client-desktop/src/desktop/native.rs', 'client-desktop/src/composition.rs',
        'client-desktop/src/desktop/composition_cleanup.rs',
        'client-desktop/src/desktop/physical_callbacks.rs',
        'client-desktop/examples/corpus_support/composition.rs',
        'adapters/sdk/src/gtk_composition.rs', 'adapters/gtk4/composition.py',
        'client-desktop/src/interaction.rs', 'client-desktop/src/renderer.rs',
        'transport/src/client.rs', 'transport/src/server.rs',
        'transport/src/source_delivery.rs', 'transport/src/producer.rs',
    ]
    report['source_file_fingerprints'] = {
        name: hashlib.sha256(Path(name).read_bytes()).hexdigest() for name in implementation
    }
    report['probe_binary_sha256'] = hashlib.sha256(args.probe.read_bytes()).hexdigest()
    names = ['gtk4', 'cairo', 'pango', 'fontconfig']
    versions = subprocess.check_output(['pkg-config', '--modversion', *names], text=True).splitlines()
    report['native_dependencies'] = dict(zip(names, versions))
    report['native_dependencies']['xvfb_binary_sha256'] = hashlib.sha256(Path(args.xvfb).read_bytes()).hexdigest()
    report['isolated_environment'] = {
        'source_display': 'separate authenticated private Xvfb1280x1024',
        'mirror_display': 'authenticated private Xvfb2300x1100; two disjoint1024x640 canvases',
        'session_bus': 'owned EXTERNAL bus with no service search directories',
        'backend': 'GTK IM simple (isolated test pin); GDK x11; GSK cairo; GIO local; portals/a11y autoactivation disabled',
    }
    report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + '\n')
    print(json.dumps({'actual_after_paint_frames': 10 if args.scenario == 'composition' else 4, 'native_captures': len(references),
                      'interior_bad_pixels': sum(c['pixels']['interior_bad_pixels'] for c in references),
                      'native_glyphs': sum(c['geometry']['glyphs_checked'] for c in references),
                      'host_memory_after_disposal': report['host_memory_after_disposal']}, sort_keys=True))


if __name__ == '__main__':
    main()
