#!/usr/bin/env python3
"""Fetch pinned, attributed public conformance assets; never used during ordinary import."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=Path, default=ROOT / 'target/corpus')
parser.add_argument('--verify-only', action='store_true')
args = parser.parse_args()
manifest = json.loads((ROOT / 'fixtures/corpus/manifest.json').read_text())
total = 0
for asset in manifest['assets']:
    for record in asset['files']:
        relative = PurePosixPath(record['path'])
        if relative.is_absolute() or '..' in relative.parts:
            raise SystemExit('Unsafe manifest path')
        destination = args.output / str(relative)
        def valid():
            return destination.is_file() and destination.stat().st_size == record['bytes'] and hashlib.sha256(destination.read_bytes()).hexdigest() == record['sha256']
        if not valid():
            if args.verify_only:
                raise SystemExit(f'Missing or changed corpus file: {destination}')
            destination.parent.mkdir(parents=True, exist_ok=True)
            with urllib.request.urlopen(record['url'], timeout=45) as response:
                data = response.read(record['bytes'] + 1)
            if len(data) != record['bytes'] or hashlib.sha256(data).hexdigest() != record['sha256']:
                raise SystemExit(f'Corpus integrity failure: {record["path"]}')
            with tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as temporary:
                temporary.write(data)
                temporary_path = Path(temporary.name)
            temporary_path.replace(destination)
        total += record['bytes']
print(json.dumps({'assets': len(manifest['assets']), 'verified_bytes': total, 'output': str(args.output)}))
