#!/usr/bin/env python3
"""Compile and run a real C consumer against an already-built toolbox library."""
import argparse
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('library', type=Path)
parser.add_argument('--compiler')
parser.add_argument('--build', type=Path, default=ROOT / 'target/c-consumer')
args = parser.parse_args()
library = args.library.resolve()
command = ['cmake', '-S', str(ROOT / 'examples/c'), '-B', str(args.build),
           '-DCMAKE_BUILD_TYPE=Release', f'-DTOOLBOX_LIBRARY={library}']
if args.compiler:
    command.append(f'-DCMAKE_C_COMPILER={args.compiler}')
subprocess.run(command, check=True)
subprocess.run(['cmake', '--build', str(args.build), '--config', 'Release'], check=True)
environment = dict(os.environ)
environment['PATH'] = str(library.parent) + os.pathsep + environment.get('PATH', '')
subprocess.run(['ctest', '--test-dir', str(args.build), '-C', 'Release', '--output-on-failure'], env=environment, check=True)
