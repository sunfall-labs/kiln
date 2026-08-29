#!/usr/bin/env bash
#
# Bundle each Effect program in src/ to a single self-contained ESM file in bundle/.
#
# Why bundle at all: the original corpus targeted a compiler with no module resolution, so the
# whole Effect library had to be inlined into one file. Kiln will want its own module loader
# eventually, but the bundles stay useful for a different reason - each is ~200 KB of real,
# machine-generated JavaScript, which is a far better parser and frontend input than anything
# written by hand.
#
# The bundles are NOT committed (see .gitignore); they are 2.5 MB of near-duplicate output
# because every one of them inlines the same library. This script plus package-lock.json is
# what makes them reproducible. Exactly one bundle is committed as a fixture - see README.md.
#
# Adapted from the source corpus's effect/build.sh: the two hardcoded absolute paths to a
# particular laptop are gone, replaced by resolution against this directory's own node_modules,
# still overridable through the same two environment variables.
set -euo pipefail

DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

NODE_MODULES=${NODE_MODULES:-$DIR/node_modules}
ESBUILD=${ESBUILD:-$NODE_MODULES/.bin/esbuild}
[[ -x $ESBUILD ]] || ESBUILD=$(command -v esbuild || true)

if [[ ! -d $NODE_MODULES/effect ]]; then
	echo "no 'effect' package under $NODE_MODULES" >&2
	echo "run 'npm ci' in $DIR, or point NODE_MODULES at a tree that has one" >&2
	exit 1
fi
if [[ -z $ESBUILD || ! -x $ESBUILD ]]; then
	echo "no esbuild; run 'npm ci' in $DIR, or set ESBUILD" >&2
	exit 1
fi

# esbuild resolves bare imports by walking up from each entry point, so src/ needs a
# node_modules to find. A symlink keeps the real one in one place; it is gitignored.
mkdir -p "$DIR/bundle"
ln -sfn "$NODE_MODULES" "$DIR/src/node_modules"

for f in "$DIR"/src/*.ts; do
	"$ESBUILD" "$f" --bundle --format=esm --platform=neutral \
		--outfile="$DIR/bundle/$(basename "$f" .ts).js" --log-level=error
done

echo "bundled $(find "$DIR/bundle" -name '*.js' | wc -l | tr -d ' ') programs into $DIR/bundle"
