#!/usr/bin/env bash
#
# Front door for the npm package compatibility corpus.
# Informational only: this is not part of the gate and not run in CI.
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
exec "$REPO_ROOT/conformance/npm-compat/run.sh" "$@"
