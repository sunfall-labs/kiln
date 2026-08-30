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
[[ -f package.json ]] || echo '{"name":"kiln-thirdparty-scratch","private":true}' >package.json
npm install --no-audit --no-fund zod immer date-fns js-yaml semver mustache marked uuid acorn lodash dayjs chalk ms >/dev/null

for probe in "$DIR"/probes/*.mjs; do
	name=$(basename "$probe" .mjs)
	cp "$probe" "$SCRATCH/$name.entry.mjs"
	# The neutral platform configures no main fields at all, so a CommonJS-only
	# package resolves to nothing without them; `module` stays first so ESM wins.
	"$ESBUILD" --bundle --format=esm --platform=neutral --target=es2022 \
		--main-fields=module,main --conditions=import,default \
		--outfile="$DIR/$name.bundle.js" "$SCRATCH/$name.entry.mjs"
	rm -f "$SCRATCH/$name.entry.mjs"
	echo "bundled $name"
done
