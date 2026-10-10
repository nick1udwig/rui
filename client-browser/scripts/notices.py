#!/usr/bin/env python3
"""Conservatively retain notices for the locked WASM build dependency closure.

Build/proc-macro dependencies are included as well as runtime dependencies. The
compiler's complete standard-library notice is retained because the linker does
not expose a reliable per-source license inclusion map. No license is assigned
to the RUI project; this does not authorize public distribution.
"""
import hashlib
from html.parser import HTMLParser
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
metadata = json.loads(subprocess.check_output([
    'cargo', 'metadata', '--locked', '--format-version', '1',
    '--filter-platform', 'wasm32-unknown-unknown'], cwd=ROOT))
packages = {p['id']: p for p in metadata['packages']}
nodes = {p['id']: p for p in metadata['resolve']['nodes']}
pending = [p['id'] for p in packages.values() if p['name'] == 'rui-client-wasm']
seen = set()
while pending:
    identity = pending.pop()
    if identity in seen:
        continue
    seen.add(identity)
    pending.extend(nodes[identity]['dependencies'])

parts = ['RUI browser package third-party notices\n\n'
         'RUI itself has no declared project license. Package metadata is '
         'UNLICENSED and private; these notices grant no license to RUI.\n'
         'The following locked WASM build closure conservatively includes '
         'build/proc-macro dependencies. npm development tools, GTK and font '
         'fixtures are not shipped in the runtime tarball.\n']
manifest = []
for identity in sorted(seen, key=lambda i: (packages[i]['name'], packages[i]['version'])):
    package = packages[identity]
    if package['source'] is None:
        continue
    directory = Path(package['manifest_path']).parent
    files = sorted(p for p in directory.iterdir() if p.is_file()
                   and p.name.upper().startswith(('LICENSE', 'COPYING', 'NOTICE')))
    if not files or not package['license']:
        raise ValueError('missing dependency license/notice: ' + package['name'])
    parts.append(f"\n{'=' * 72}\n{package['name']} {package['version']}\n"
                 f"Declared license: {package['license']}\n")
    item = {'name': package['name'], 'version': package['version'],
            'license': package['license'], 'files': {}}
    for path in files:
        data = path.read_bytes()
        item['files'][path.name] = hashlib.sha256(data).hexdigest()
        parts.append('\n--- ' + path.name + ' ---\n' + data.decode('utf-8'))
    manifest.append(item)

class Plain(HTMLParser):
    def __init__(self):
        super().__init__(); self.parts = []
    def handle_data(self, data):
        self.parts.append(data)

sysroot = Path(subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip())
documentation = sysroot / 'share/doc/rust'
parser = Plain(); parser.feed((documentation / 'COPYRIGHT-library.html').read_text())
parts.append('\n' + '=' * 72 + '\nRust standard library (complete conservative notices)\n'
             + ''.join(parser.parts))
for path in sorted((documentation / 'licenses').glob('*.txt')):
    parts.append('\n--- Rust license ' + path.name + ' ---\n' + path.read_text())
package_root = ROOT / 'client-browser'
(package_root / 'THIRD_PARTY_NOTICES.txt').write_text('\n'.join(parts) + '\n')
(package_root / 'scripts/licenses.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(json.dumps({'lockedDependencies': len(manifest), 'rustc':
      subprocess.check_output(['rustc', '--version'], text=True).strip()}))
