#!/usr/bin/env bash
# Fetch the pinned Web Platform Tests commit (sparse: the Minimum Common API's directories) into
# the durable shared cache.
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
python3 "$(kiln_repo_root)/scripts/wpt.py" fetch
