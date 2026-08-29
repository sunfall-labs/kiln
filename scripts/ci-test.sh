#!/usr/bin/env bash
#
# Run the workspace test suite.
#
# cargo-nextest is the better runner - process-per-test, so a panicking test cannot take its
# neighbours with it, and much clearer failure output - and it is what developers have
# installed locally. It is not on a stock GitHub runner, and for a workspace this small the
# install costs more wall clock than the runner saves, so CI takes the fallback path on
# purpose. The two branches must cover the same tests: nextest does not execute doctests, so
# that branch runs them separately.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

# --locked everywhere: a manifest change that was not accompanied by a Cargo.lock update is a
# reproducibility bug, and CI is the right place to notice it.
if cargo nextest --version >/dev/null 2>&1; then
	echo "test runner: cargo-nextest"
	cargo nextest run --workspace --locked
	cargo test --workspace --locked --doc
else
	echo "test runner: cargo test (cargo-nextest is not installed)"
	cargo test --workspace --locked
fi
