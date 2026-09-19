#!/usr/bin/env bash
# Real-world programs, every runtime, one table. See README.md.
set -euo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
exec python3 "$HERE/compare.py" "$@"
