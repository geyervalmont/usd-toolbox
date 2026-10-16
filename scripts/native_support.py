"""Locate target support libraries supplied by the selected Rust toolchain."""
from pathlib import Path
import subprocess


def musl_unwind_library(target: str) -> Path:
    if 'linux-musl' not in target:
        raise ValueError(f'Expected a Rust musl target triple, got {target!r}; '
                         'pass --unwind-library for a custom library layout')
    directory = subprocess.check_output(
        ['rustc', '--print', 'target-libdir', '--target', target], text=True).strip()
    archive = Path(directory) / 'self-contained' / 'libunwind.a'
    if not archive.is_file():
        raise FileNotFoundError(f'Missing Rust musl unwind archive: {archive}; '
                                f'install rust-std for {target}')
    return archive
