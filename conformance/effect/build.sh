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

LOCK_DIR="$DIR/bundle/.kiln-build.lock"
if ! mkdir "$LOCK_DIR" 2>/dev/null; then
	echo "another Effect bundle build is active (or left $LOCK_DIR behind)" >&2
	exit 1
fi

TEMP_FILES=()
cleanup() {
	if [[ ${#TEMP_FILES[@]} -ne 0 ]]; then
		rm -f -- "${TEMP_FILES[@]}"
	fi
	rmdir "$LOCK_DIR" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

write_frames() {
	local output=$1
	shift
	node --input-type=module - "$output" "$@" <<'NODE'
import fs from "node:fs";

const [, , output, ...inputs] = process.argv;
const chunks = [Buffer.from("kiln-bundle-provenance-v2\n")];
for (const input of inputs) {
	const bytes = fs.readFileSync(input);
	chunks.push(Buffer.from(`${bytes.length}\n`), bytes);
}
fs.writeFileSync(output, Buffer.concat(chunks));
NODE
}

write_bundle_provenance() {
	local output=$1 bundle=$2 input_snapshot=$3
	node --input-type=module - "$output" "$bundle" "$input_snapshot" <<'NODE'
import fs from "node:fs";

const [, , output, bundlePath, snapshotPath] = process.argv;
const header = Buffer.from("kiln-bundle-provenance-v2\n");
const bundle = fs.readFileSync(bundlePath);
const snapshot = fs.readFileSync(snapshotPath);
if (!snapshot.subarray(0, header.length).equals(header)) {
	throw new Error("invalid bundle input snapshot");
}
fs.writeFileSync(
	output,
	Buffer.concat([
		header,
		Buffer.from(`${bundle.length}\n`),
		bundle,
		snapshot.subarray(header.length),
	]),
);
NODE
}

for f in "$DIR"/src/*.ts; do
	name=$(basename "$f" .ts)
	output="$DIR/bundle/$name.js"
	bundle_temp="$DIR/bundle/.$name.$$.tmp.js"
	provenance_temp="$DIR/bundle/.$name.$$.tmp.inputs"
	provenance_before="$DIR/bundle/.$name.$$.before.inputs"
	provenance_check="$DIR/bundle/.$name.$$.check.inputs"
	TEMP_FILES+=(
		"$bundle_temp" "$provenance_temp" "$provenance_before" "$provenance_check"
	)
	inputs=("$f" "$DIR/package.json" "$DIR/package-lock.json" "$DIR/build.sh")

	# Snapshot the complete declared input before esbuild reads it, then prove nothing changed while
	# it ran. The bundle is renamed first and the authorizing provenance last, so interruption can
	# never leave a partial or stale output that the conformance runner accepts.
	write_frames "$provenance_before" "${inputs[@]}"
	"$ESBUILD" "$f" --bundle --format=esm --platform=neutral \
		--outfile="$bundle_temp" --log-level=error
	write_frames "$provenance_check" "${inputs[@]}"
	if ! cmp -s "$provenance_before" "$provenance_check"; then
		echo "bundle inputs changed while building $f; retry" >&2
		exit 1
	fi
	write_bundle_provenance "$provenance_temp" "$bundle_temp" "$provenance_before"
	mv -f -- "$bundle_temp" "$output"
	mv -f -- "$provenance_temp" "$output.kiln-inputs"
done

echo "bundled $(find "$DIR/bundle" -name '*.js' | wc -l | tr -d ' ') programs into $DIR/bundle"
