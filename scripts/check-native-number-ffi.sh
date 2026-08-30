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
