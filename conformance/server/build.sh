#!/usr/bin/env bash
#
# Install the frameworks the two ecosystem probes import by bare specifier.
#
# `s04-fastify` and `s05-express` are the gate that says a real framework runs, not a hand-written
# approximation of one, so they import `fastify` and `express` the way an application would. That
# makes them the only programs in this corpus with an input that is not in the repository:
# `packages/node_modules` is gitignored, and `package-lock.json` beside it is what makes the
# install reproducible.
#
# Until it has been run, both probes report `needs-packages` and are skipped with a reason rather
# than failing -- a missing dependency is not a wrong answer. This script exists so that reason can
# name a command instead of describing one, which is the same shape as `conformance/effect` and
# `conformance/thirdparty`.
set -euo pipefail

DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/packages"

if ! command -v npm >/dev/null 2>&1; then
	echo "npm is not on PATH; it is what installs the pinned frameworks in $DIR" >&2
	exit 1
fi

# `npm ci` rather than `npm install`: it installs exactly package-lock.json and fails if the two
# disagree, so a probe cannot quietly start testing a different version of a framework than the
# one this corpus pinned.
npm ci --prefix "$DIR" --silent

printf 'installed %s\n' "$DIR/node_modules"
printf 'run: kiln test conformance/server --node\n'
