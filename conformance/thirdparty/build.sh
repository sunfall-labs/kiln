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
# The original set: small, self-contained libraries.
npm install --no-audit --no-fund zod immer date-fns js-yaml semver mustache marked uuid acorn \
	lodash dayjs chalk ms >/dev/null

# The heavy set: large machine-generated applications rather than utility libraries. `typescript`
# is pinned to 5.x on purpose -- 7.x is the Go port and ships a native binary, not JavaScript, so
# it has nothing to run here.
npm install --no-audit --no-fund typescript@5.9 @vue/reactivity @babel/parser @babel/generator \
	@babel/traverse postcss rxjs react react-dom mobx ajv p-queue p-map p-limit p-retry p-timeout \
	ix >/dev/null

# Four probes reach a library that requires a Node builtin -- `path`, `fs`, `os`, `util`,
# `async_hooks`. Those cannot be bundled for the neutral platform at all, and bundling them as
# *ESM* for the node platform produces the `createRequire(import.meta.url)` preamble every real
# node build emits, which kiln refuses at `import.meta`. So they are bundled as CommonJS, which
# is what most published Node builds still ship and what both node and kiln can actually load.
# Their builtin requires then survive as literal `require("path")` calls, which is what kiln's
# static module graph needs to see (findings/require-alias.js records why that matters).
NODE_PLATFORM=" typescript babel postcss react-ssr "

for probe in "$DIR"/probes/*.mjs; do
	name=$(basename "$probe" .mjs)
	cp "$probe" "$SCRATCH/$name.entry.mjs"
	if [[ $NODE_PLATFORM == *" $name "* ]]; then
		"$ESBUILD" --bundle --format=cjs --platform=node --target=es2022 \
			--outfile="$DIR/$name.bundle.js" "$SCRATCH/$name.entry.mjs"
	else
		# The neutral platform configures no main fields at all, so a CommonJS-only
		# package resolves to nothing without them; `module` stays first so ESM wins.
		"$ESBUILD" --bundle --format=esm --platform=neutral --target=es2022 \
			--main-fields=module,main --conditions=import,default \
			--outfile="$DIR/$name.bundle.js" "$SCRATCH/$name.entry.mjs"
	fi
	rm -f "$SCRATCH/$name.entry.mjs"
	echo "bundled $name"
done
