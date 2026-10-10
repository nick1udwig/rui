#!/usr/bin/env python3
"""Build browser ES modules and native-oracle fixtures for the WASM spike."""
import argparse
import gzip
import json
import os
from pathlib import Path
import shutil
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parent
REPOSITORY = ROOT.parent


def run(*command, **kwargs):
    return subprocess.run(command, cwd=REPOSITORY, check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--wasm-bindgen', default=os.environ.get('WASM_BINDGEN', shutil.which('wasm-bindgen')))
    args = parser.parse_args()
    if not args.wasm_bindgen:
        parser.error('install the matching wasm-bindgen-cli or set WASM_BINDGEN')
    expected = tomllib.loads((ROOT / 'Cargo.toml').read_text())['dependencies']['wasm-bindgen'].removeprefix('=')
    version = subprocess.check_output([args.wasm_bindgen, '--version'], text=True).strip()
    if version != f'wasm-bindgen {expected}':
        parser.error(f'expected wasm-bindgen-cli {expected}, got {version}')
    metadata = json.loads(run('cargo', 'metadata', '--locked', '--no-deps', '--format-version', '1', capture_output=True, text=True).stdout)
    target = Path(metadata['target_directory'])
    run('cargo', 'build', '--locked', '--release', '-p', 'rui-client-wasm', '--target', 'wasm32-unknown-unknown')
    package = ROOT / 'pkg'
    run(args.wasm_bindgen, str(target / 'wasm32-unknown-unknown/release/rui_client_wasm.wasm'), '--target', 'web', '--out-name', 'rui_client_wasm', '--out-dir', str(package))
    browser_package = REPOSITORY / 'client-browser/wasm'
    if browser_package.parent.exists():
        browser_package.mkdir(exist_ok=True)
        for artifact in package.glob('rui_client_wasm*'):
            shutil.copy2(artifact, browser_package / artifact.name)
    # Reuse the existing golden trace and native reducer as the independent
    # execution-target baseline. No expected state is inferred in JavaScript.
    golden = (REPOSITORY / 'conformance/golden/semantic.jsonl').read_text()
    run('cargo', 'build', '--locked', '--release', '-p', 'rui-protocol-core', '--example', 'replay')
    baseline = run(str(target / 'release/examples/replay'), '--full-state', input=golden, text=True, capture_output=True)
    requests = [json.loads(line) for line in golden.splitlines()]
    responses = [json.loads(line) for line in baseline.stdout.splitlines()]
    assert len(requests) == len(responses)
    fixtures = []
    for request, response in zip(requests, responses):
        state = response['state']
        records = [next(iter(record.values())) for record in state['records'].values()]
        fixtures.append({**request, 'accepted': response['accepted'], 'revision': str(state['revision']),
                         'records': [{'id': str(record['id']), 'version': str(record['version'])} for record in records]})
    (package / 'fixtures.json').write_text(json.dumps(fixtures) + '\n')
    import importlib.util
    contract_module = importlib.util.spec_from_file_location('rui_contract', ROOT / 'tests/contract.py')
    contract = importlib.util.module_from_spec(contract_module)
    import sys
    sys.path.insert(0, str(REPOSITORY))
    contract_module.loader.exec_module(contract)
    complete_model = contract.fixture(target / 'release/examples/replay')
    for destination in (package, browser_package):
        (destination / 'contract.json').write_text(json.dumps(complete_model) + '\n')

    body = (package / 'rui_client_wasm_bg.wasm').read_bytes()
    print(json.dumps({'wasm_bytes': len(body), 'wasm_gzip_bytes': len(gzip.compress(body, mtime=0)), 'native_oracle_prefixes': len(fixtures)}))


if __name__ == '__main__':
    main()
