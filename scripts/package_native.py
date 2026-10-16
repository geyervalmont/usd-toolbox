#!/usr/bin/env python3
"""Package already-built native binaries with headers, examples and checksums."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import tomllib
from dependency_notices import collect

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--target', required=True)
parser.add_argument('--out', type=Path, default=ROOT / 'target/dist')
args = parser.parse_args()
version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
name = f'usd-toolbox-{version}-{args.target}'
output = args.out.resolve()
package = output / name
package.mkdir(parents=True, exist_ok=True)
binaries = ROOT / 'target' / args.target / 'release'
required = ['usd-toolbox.exe', 'usd_toolbox_ffi.dll', 'usd_toolbox_ffi.dll.lib', 'usd_toolbox_ffi.lib'] if 'windows' in args.target else (
    ['usd-toolbox', 'libusd_toolbox_ffi.a'] + ([] if 'musl' in args.target else ['libusd_toolbox_ffi.dylib' if 'apple' in args.target else 'libusd_toolbox_ffi.so']))
for filename in required:
    shutil.copyfile(binaries / filename, package / filename)
    shutil.copymode(binaries / filename, package / filename)
shutil.copyfile(ROOT / 'crates/ffi/include/usd_toolbox.h', package / 'usd_toolbox.h')
for filename in ['README.md', 'todo.md']:
    shutil.copyfile(ROOT / filename, package / filename)
# Preserve documentation paths so relative links also work inside an extracted bundle.
for directory in ['docs', 'examples', 'scripts']:
    shutil.copytree(ROOT / directory, package / directory, dirs_exist_ok=True,
                    ignore=shutil.ignore_patterns('__pycache__', 'node_modules', 'dist'))
for directory in ['crates/gltf', 'packages/web']:
    destination = package / directory
    destination.mkdir(parents=True, exist_ok=True)
    for source in (ROOT / directory).glob('*.md'):
        shutil.copyfile(source, destination / source.name)
collect(package / 'THIRD_PARTY_NOTICES')
(package / 'release.json').write_text(json.dumps({'schema': 'usd-toolbox.release.v1', 'version': version,
    'target': args.target, 'distribution': 'project distribution licence is not declared'}, indent=2) + '\n')
(package / 'SHA256SUMS').write_text(''.join(
    f'{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.relative_to(package).as_posix()}\n'
    for path in sorted(package.rglob('*')) if path.is_file() and path.name != 'SHA256SUMS'))
archive = shutil.make_archive(str(output / name), 'zip' if 'windows' in args.target else 'gztar', output, name)
Path(archive + '.sha256').write_text(f'{hashlib.sha256(Path(archive).read_bytes()).hexdigest()}  {Path(archive).name}\n')
print(archive)
