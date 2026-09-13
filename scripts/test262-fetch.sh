#!/usr/bin/env bash
# Fetch the pinned Test262 commit into the durable shared cache.
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
REPO_ROOT="$(kiln_repo_root)"
python3 "$REPO_ROOT/scripts/test262_corpus.py" fetch
