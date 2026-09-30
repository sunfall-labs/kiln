#!/usr/bin/env bash
#
# Run the complete repository conformance corpus through kiln's public one-shot CLI.
#
# scripts/ci-conformance.sh owns the results schema and invokes runners as
# `conformance/run.sh --out <path>`. Keep this wrapper deliberately thin: the CLI is the
# implementation of discovery, execution, reporting, and the exit status, while this script
# only makes that public invocation reproducible from any working directory.
set -euo pipefail

if [[ $# -ne 2 || $1 != --out || -z $2 ]]; then
	echo "usage: conformance/run.sh --out <path>" >&2
	exit 2
fi

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

exec cargo run -p kiln-cli --bin kiln --locked -- --one-shot test conformance --out "$2"
