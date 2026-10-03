#!/usr/bin/env python3
"""Retain exact isolated GTK TLS traces; raster diagnostics stay in run dirs."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil


def main():
    parser = argparse.ArgumentParser()
    for name in ('full-visual', 'disconnect', 'revocation'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = vars(parser.parse_args())
    target = Path(__file__).parent
    staging = target / 'transport-golden.next'
    if staging.exists() or (target / 'transport-golden').exists():
        raise ValueError('fresh retained trace directory required')
    report = {'pinned_gtk': [4, 22, 4], 'scenarios': {},
              'presentation_scope': 'Explicit driver-supplied simulated Presented callbacks; no actual mirror after-paint evidence.',
              'observer_scope': 'Two real TLS clients use separate instances of the same Rust receiver; independent protocol audit is a separate gate.',
              'projection_policy': 'Approved viewers receive the same public controlled corpus, with independent action permissions; no redacted observer projection claim.',
              'retained_exact_trace': 'fixtures/gtk/transport-golden'}
    total = 0
    for name in ('full-visual', 'disconnect', 'revocation'):
        run = args[name.replace('-', '_')]
        source = run / 'interaction.json'
        if source.stat().st_size > 65536:
            raise ValueError('report ceiling')
        evidence = json.loads(source.read_text())
        if evidence['mode'] != name or evidence['host_memory_after_disposal'] != 0:
            raise ValueError('scenario or disposal mismatch')
        destination = staging / name
        destination.mkdir(parents=True)
        for subtree in ('source-vector', 'protocol'):
            files = list((run / subtree).rglob('*'))
            if len(files) > 4096:
                raise ValueError('retained file ceiling')
            for path in files:
                if not path.is_file():
                    continue
                if path.is_symlink() or path.suffix not in ('.bin', '.json', '.rvp'):
                    raise ValueError('unexpected retained asset')
                total += path.stat().st_size
                if total > 16 << 20:
                    raise ValueError('finite exact artifact ceiling')
            shutil.copytree(run / subtree, destination / subtree)
        report['scenarios'][name] = evidence
    report['retained_trace_sha256'] = {
        str(path.relative_to(staging)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(staging.rglob('*')) if path.is_file()
    }
    report['retained_bytes'] = total
    staging.rename(target / 'transport-golden')
    (target / 'transport-interaction.json').write_text(json.dumps(report, indent=2, sort_keys=True) + '\n')
    print(f'retained {len(report["retained_trace_sha256"])} exact indexed transport/vector files')


if __name__ == '__main__':
    main()
