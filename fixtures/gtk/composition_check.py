#!/usr/bin/env python3
"""Actual pinned TLS/native composition on owned displays and service-free bus."""
import argparse
import hashlib
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from fixtures.gtk.mirror_check import run_private


def digest(path, ceiling=64 << 20):
    if path.stat().st_size > ceiling:
        raise ValueError('composition-evidence-file-ceiling')
    value = hashlib.sha256()
    with path.open('rb') as handle:
        while block := handle.read(1 << 20):
            value.update(block)
    return value.hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--xvfb', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--scenario', choices=['commit', 'cancel', 'focus', 'selection', 'removal', 'revocation', 'disconnect', 'renewal', 'expiry', 'clock-insertion', 'clock-deletion', 'clock-noop', 'clock-empty', 'clock-prefix', 'clock-empty-noop'], default='commit')
    args = parser.parse_args()
    if args.output.exists() and any(args.output.iterdir()):
        raise ValueError('fresh owned output directory required')
    stdout = run_private(args.xvfb, ['target/debug/gtk_composition_check', str(args.output), args.scenario])
    report = args.output / 'interaction.json'
    evidence = json.loads(report.read_text())
    evidence['retained_exact_trace_sha256'] = {
        str(path.relative_to(args.output)): digest(path)
        for directory in ('source-vector', 'protocol-before', 'protocol-final')
        for path in sorted((args.output / directory).iterdir()) if path.is_file()
    }
    implementation = [
        'adapters/sdk/Cargo.toml', 'adapters/sdk/src/gtk_host.rs', 'adapters/sdk/src/gtk_port.rs',
        'adapters/sdk/src/gtk_composition.rs', 'adapters/sdk/src/bin/gtk_composition_check.rs',
        'adapters/gtk4/native.py', 'adapters/gtk4/composition.py', 'adapters/gtk4/state.py',
        'adapters/sdk/producer.py', 'fixtures/gtk/whole_view.py', 'fixtures/gtk/whole_native_source.py',
        'fixtures/gtk/composition_check.py', 'fixtures/gtk/mirror_check.py',
        'protocol-core/src/input.rs', 'protocol-core/src/commands.rs', 'protocol-core/src/session.rs',
    ]
    evidence['implementation_sha256'] = {path: digest(Path(path)) for path in implementation}
    evidence['probe_binary_sha256'] = digest(Path('target/debug/gtk_composition_check'), 128 << 20)
    evidence['diagnostic_scope'] = 'Explicit controlled-fixture controller/observer post-auth diagnostics; private preedit body retained only under controller scope; no credentials or observer preedit.'
    report.write_text(json.dumps(evidence, indent=2) + '\n')
    print(stdout.strip())


if __name__ == '__main__':
    main()
