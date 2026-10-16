#!/usr/bin/env python3
"""Compile and run a real C consumer against an already-built toolbox library."""
import argparse
import os
from pathlib import Path
import subprocess
from native_support import musl_unwind_library

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('library', type=Path)
parser.add_argument('--compiler')
parser.add_argument('--unwind-library', type=Path,
                    help='Explicit compatible unwind archive for a static musl build')
parser.add_argument('--build', type=Path, default=ROOT / 'target/c-consumer')
args = parser.parse_args()
library = args.library.resolve()
command = ['cmake', '-S', str(ROOT / 'examples/c'), '-B', str(args.build),
           '-DCMAKE_BUILD_TYPE=Release', f'-DTOOLBOX_LIBRARY={library}']
if args.compiler:
    command.append(f'-DCMAKE_C_COMPILER={args.compiler}')
unwind = args.unwind_library
if (unwind is None and args.compiler and 'musl' in Path(args.compiler).name
        and not (library.parent / 'libunwind.a').is_file()):
    # Cargo output is target/<triple>/release/<library>. Extracted packages
    # carry their own archive and do not require Rust to be installed.
    unwind = musl_unwind_library(library.parent.parent.name)
if unwind is not None:
    command.append(f'-DTOOLBOX_UNWIND_LIBRARY={unwind.resolve()}')
subprocess.run(command, check=True)
subprocess.run(['cmake', '--build', str(args.build), '--config', 'Release'], check=True)
environment = dict(os.environ)
environment['PATH'] = str(library.parent) + os.pathsep + environment.get('PATH', '')
subprocess.run(['ctest', '--test-dir', str(args.build), '-C', 'Release', '--output-on-failure'], env=environment, check=True)
