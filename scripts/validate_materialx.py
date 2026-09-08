#!/usr/bin/env python3
"""Validate an exported document with the official MaterialX bindings."""

from pathlib import Path
import sys

try:
    import MaterialX as mx
except ImportError as error:
    raise SystemExit("MaterialX Python bindings are missing") from error


def main() -> int:
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} path/to/file.mtlx", file=sys.stderr)
        return 2

    path = Path(sys.argv[1])
    document = mx.createDocument()
    try:
        mx.readFromXmlFile(document, str(path))
    except Exception as error:
        print(f"MaterialX could not read {path}: {error}", file=sys.stderr)
        return 1

    libraries = mx.createDocument()
    mx.loadLibraries(
        mx.getDefaultDataLibraryFolders(),
        mx.getDefaultDataSearchPath(),
        libraries,
    )
    document.importLibrary(libraries)
    valid, message = document.validate()
    if not valid:
        print(message, file=sys.stderr)
        return 1
    print(f"MaterialX {mx.__version__} accepted {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
