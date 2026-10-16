#!/usr/bin/env python3
"""Collect pinned Rust dependency licences without choosing a project licence."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]

def collect(destination: Path):
    destination.mkdir(parents=True, exist_ok=True)
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--format-version', '1'], cwd=ROOT))
    members = set(metadata['workspace_members'])
    records = []
    for package in sorted(metadata['packages'], key=lambda item: (item['name'], item['version'])):
        if package['id'] in members:
            continue
        folder = Path(package['manifest_path']).parent
        # Crates use mixed-case filenames; some (notably r-efi) put the full
        # permission and copyright notices in AUTHORS rather than LICENSE.
        prefixes = ('license', 'licence', 'copying', 'notice', 'authors')
        files = {path for path in folder.iterdir()
                 if path.is_file() and path.name.lower().startswith(prefixes)}
        if package.get('license_file'):
            path = folder / package['license_file']
            if path.is_file():
                files.add(path)
        if not files:
            raise RuntimeError(
                f"Missing dependency notices for {package['name']} {package['version']} "
                f"({package['repository']}); restore the upstream notice before packaging"
            )
        relative = f"{package['name']}-{package['version']}"
        target = destination / relative
        target.mkdir(exist_ok=True)
        for path in sorted(files):
            shutil.copyfile(path, target / path.name)
        records.append({'name': package['name'], 'version': package['version'], 'license': package['license'],
                        'repository': package['repository'], 'notices': [f'{relative}/{p.name}' for p in sorted(files)]})
    (destination / 'index.json').write_text(json.dumps(records, indent=2) + '\n')
    (destination / 'README.txt').write_text(
        'Dependency licences for the locked workspace, including build/test dependencies.\n'
        'This inventory is intentionally broader than the compiled runtime.\n'
        'The project itself has no declared distribution licence; these notices do not grant one.\n')

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('destination', type=Path)
    collect(parser.parse_args().destination)
