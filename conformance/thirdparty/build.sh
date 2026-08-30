#!/usr/bin/env bash
#
# Rebuild the third-party bundles from the probe sources in this directory.
#
# The libraries are installed into a scratch directory rather than being vendored: what is
# committed is the *bundle*, so a rebuild is only needed when a probe changes or a library
# version is deliberately moved.
set -euo pipefail

DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
SCRATCH="${SCRATCH:-$DIR/.scratch}"
ESBUILD="${ESBUILD:-$DIR/../effect/node_modules/.bin/esbuild}"
[[ -x $ESBUILD ]] || ESBUILD=$(command -v esbuild || true)
if [[ -z $ESBUILD || ! -x $ESBUILD ]]; then
	echo "no esbuild; run 'npm ci' in conformance/effect, or set ESBUILD" >&2
	exit 1
fi

mkdir -p "$SCRATCH"
cd "$SCRATCH"
[[ -f package.json ]] || npm init -y >/dev/null
npm install --no-audit --no-fund zod immer date-fns js-yaml semver mustache marked uuid >/dev/null

for probe in "$DIR"/probes/*.mjs; do
	name=$(basename "$probe" .mjs)
	"$ESBUILD" --bundle --format=esm --platform=neutral --target=es2022 \
		--outfile="$DIR/$name.bundle.js" "$probe"
	echo "bundled $name"
done
