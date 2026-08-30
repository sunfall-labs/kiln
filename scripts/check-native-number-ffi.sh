#!/usr/bin/env bash
#
# Package the no-std number provider exactly as it ships, then call that archive from strict C.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

KILN_NUMBER_CC="${CC:-cc}"
if ! command -v "$KILN_NUMBER_CC" >/dev/null 2>&1; then
	printf 'C driver %s is not available\n' "$KILN_NUMBER_CC" >&2
	exit 1
fi

KILN_NUMBER_TEMP="$(mktemp -d "${TMPDIR:-/tmp}/kiln-number-ffi.XXXXXX")"
kiln_number_cleanup() {
	find "$KILN_NUMBER_TEMP" -type l -delete
	find "$KILN_NUMBER_TEMP" -type f -delete
	find "$KILN_NUMBER_TEMP" -depth -type d -exec rmdir {} \;
}
trap kiln_number_cleanup EXIT

bash scripts/package-native-number-provider.sh "$KILN_NUMBER_TEMP/package" >/dev/null
KILN_NUMBER_ARCHIVE="$KILN_NUMBER_TEMP/package/libkiln_native_number_ffi.a"
KILN_NUMBER_MANIFEST="$KILN_NUMBER_TEMP/package/kiln-native-number-provider.json"
if [[ ! -f $KILN_NUMBER_ARCHIVE || ! -f $KILN_NUMBER_MANIFEST ]]; then
	printf 'packaged number provider is incomplete under %s\n' "$KILN_NUMBER_TEMP/package" >&2
	exit 1
fi

"$KILN_NUMBER_CC" -std=c11 -pedantic -Wall -Wextra -Werror \
	crates/kiln-native-number-ffi/tests/ffi_harness.c \
	"$KILN_NUMBER_ARCHIVE" -lm -o "$KILN_NUMBER_TEMP/probe"
"$KILN_NUMBER_TEMP/probe"

# Exercise the same package through the product resolver/linker. The explicit manifest wins over
# source fallback, so a schema, target, digest, cache-key, or archive-link regression fails here.
env KILN_NATIVE_NUMBER_PROVIDER_MANIFEST="$KILN_NUMBER_MANIFEST" \
	cargo run --quiet --locked -p kiln-cli --bin kiln -- \
	--one-shot build examples/hello.ts --output "$KILN_NUMBER_TEMP/native-probe" >/dev/null
"$KILN_NUMBER_TEMP/native-probe" >/dev/null

# Prove the package is found from a shipped install layout, not only through the override above.
# The child has a C toolchain but cannot see Cargo or rustc, so failed installed discovery cannot
# silently fall back to rebuilding the provider from this checkout.
KILN_NUMBER_CARGO="${CARGO:-cargo}"
KILN_NUMBER_RUSTC="${RUSTC:-rustc}"
KILN_NUMBER_HOST_TARGET=$(
	"$KILN_NUMBER_RUSTC" -vV | sed -n 's/^host: //p'
)
if [[ -n ${CARGO_BUILD_TARGET:-} && $CARGO_BUILD_TARGET != "$KILN_NUMBER_HOST_TARGET" ]]; then
	printf 'installed-layout smoke requires the host target %s, not %s\n' \
		"$KILN_NUMBER_HOST_TARGET" "$CARGO_BUILD_TARGET" >&2
	exit 1
fi

"$KILN_NUMBER_CARGO" build --quiet --locked -p kiln-cli --bin kiln
KILN_NUMBER_TARGET_DIR="${CARGO_TARGET_DIR:-target}"
if [[ $KILN_NUMBER_TARGET_DIR != /* ]]; then
	KILN_NUMBER_TARGET_DIR="$(kiln_repo_root)/$KILN_NUMBER_TARGET_DIR"
fi
KILN_NUMBER_CLI_DIRECTORY=$KILN_NUMBER_TARGET_DIR
if [[ -n ${CARGO_BUILD_TARGET:-} ]]; then
	KILN_NUMBER_CLI_DIRECTORY+="/$CARGO_BUILD_TARGET"
fi
KILN_NUMBER_CLI="$KILN_NUMBER_CLI_DIRECTORY/debug/kiln"
if [[ ! -x $KILN_NUMBER_CLI ]]; then
	printf 'kiln CLI was not produced at %s\n' "$KILN_NUMBER_CLI" >&2
	exit 1
fi

KILN_NUMBER_INSTALL="$KILN_NUMBER_TEMP/install"
KILN_NUMBER_PROJECT="$KILN_NUMBER_TEMP/project"
KILN_NUMBER_TOOLS="$KILN_NUMBER_TEMP/tools"
mkdir -p "$KILN_NUMBER_INSTALL/bin" "$KILN_NUMBER_INSTALL/lib/kiln" \
	"$KILN_NUMBER_PROJECT" "$KILN_NUMBER_TOOLS"
cp "$KILN_NUMBER_CLI" "$KILN_NUMBER_INSTALL/bin/kiln"
cp "$KILN_NUMBER_ARCHIVE" "$KILN_NUMBER_MANIFEST" "$KILN_NUMBER_INSTALL/lib/kiln/"
cp examples/hello.ts "$KILN_NUMBER_PROJECT/hello.ts"

KILN_NUMBER_CC_PATH=$(command -v "$KILN_NUMBER_CC")
ln -s "$KILN_NUMBER_CC_PATH" "$KILN_NUMBER_TOOLS/cc"
for KILN_NUMBER_LINK_TOOL in as ld; do
	if KILN_NUMBER_LINK_TOOL_PATH=$(command -v "$KILN_NUMBER_LINK_TOOL" 2>/dev/null); then
		ln -s "$KILN_NUMBER_LINK_TOOL_PATH" "$KILN_NUMBER_TOOLS/$KILN_NUMBER_LINK_TOOL"
	fi
done
if PATH="$KILN_NUMBER_TOOLS" command -v cargo >/dev/null 2>&1 || \
	PATH="$KILN_NUMBER_TOOLS" command -v rustc >/dev/null 2>&1; then
	printf 'restricted installed-layout PATH unexpectedly contains Rust build tools\n' >&2
	exit 1
fi

(
	cd "$KILN_NUMBER_PROJECT"
	env -u CARGO -u RUSTC -u KILN_NATIVE_NUMBER_PROVIDER_MANIFEST \
		CC=cc PATH="$KILN_NUMBER_TOOLS" \
		"$KILN_NUMBER_INSTALL/bin/kiln" --one-shot build hello.ts \
		--output "$KILN_NUMBER_TEMP/installed-native-probe" >/dev/null
)
"$KILN_NUMBER_TEMP/installed-native-probe" >/dev/null
