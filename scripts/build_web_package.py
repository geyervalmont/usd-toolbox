#!/usr/bin/env python3
"""Build a local, versioned browser package without publishing it."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tomllib
from dependency_notices import collect

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--out', type=Path, default=ROOT / 'target/web-package')
parser.add_argument('--wasm-bindgen', default=os.environ.get('WASM_BINDGEN', 'wasm-bindgen'))
parser.add_argument('--debug', action='store_true')
parser.add_argument('--full', action='store_true', help='Include optional material authoring/target APIs')
args = parser.parse_args()
profile = 'debug' if args.debug else 'release'
command = ['cargo', 'build', '--locked', '-p', 'usd-toolbox-wasm', '--target', 'wasm32-unknown-unknown']
if not args.debug:
    command.append('--release')
if not args.full:
    command.append('--no-default-features')
subprocess.run(command, cwd=ROOT, check=True)
out = args.out.resolve()
if out == ROOT or ROOT in out.parents and out.parts[len(ROOT.parts)] != 'target':
    raise SystemExit('Output must be outside the source tree or beneath target/')
out.mkdir(parents=True, exist_ok=True)
for name in ['index.js', 'index.d.ts', 'worker.js', 'README.md']:
    shutil.copyfile(ROOT / 'packages/web' / name, out / name)
version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
package = json.loads((ROOT / 'packages/web/package.json').read_text())
package['version'] = version
package.pop('scripts', None)
(out / 'package.json').write_text(json.dumps(package, indent=2) + '\n')
collect(out / 'THIRD_PARTY_NOTICES')
subprocess.run([args.wasm_bindgen, '--target', 'web', '--out-dir', str(out / 'wasm'),
                str(ROOT / 'target/wasm32-unknown-unknown' / profile / 'usd_toolbox_wasm.wasm')], check=True)
report = {'schema': 'usd-toolbox.web-build.v1', 'version': version, 'profile': profile,
          'material_tools': args.full, 'files': {}}
for path in sorted(out.rglob('*')):
    if path.is_file() and path.name not in ['build-report.json', 'SHA256SUMS']:
        data = path.read_bytes()
        report['files'][str(path.relative_to(out))] = {
            'bytes': len(data), 'gzip_bytes': len(gzip.compress(data, mtime=0)),
            'sha256': hashlib.sha256(data).hexdigest(),
        }
(out / 'build-report.json').write_text(json.dumps(report, indent=2) + '\n')
(out / 'SHA256SUMS').write_text(''.join(f'{data["sha256"]}  {name}\n' for name, data in report['files'].items()))
print(json.dumps({'package': str(out), 'version': version, 'wasm': report['files']['wasm/usd_toolbox_wasm_bg.wasm']}))
