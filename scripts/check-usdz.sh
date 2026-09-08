#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: $0 path/to/file.usdz" >&2
    exit 2
fi

if command -v usdchecker >/dev/null 2>&1; then
    exec usdchecker "$1"
fi

echo "usdchecker not found; using the OpenUSD Python validation fallback" >&2
exec python3 "$(dirname "$0")/validate_usdz.py" "$1"
