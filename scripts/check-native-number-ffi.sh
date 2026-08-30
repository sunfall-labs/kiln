#!/usr/bin/env bash
#
# Build the no-std number provider exactly as a shipped target archive and call it from strict C.
# The ordinary workspace artifact is an rlib so test builds keep their unwind-capable harness;
# packaging deliberately applies panic=abort to the adapter and every dependency in one command.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

KILN_NUMBER_RUSTFLAGS="${RUSTFLAGS:-}"
if [[ -n $KILN_NUMBER_RUSTFLAGS ]]; then
	KILN_NUMBER_RUSTFLAGS+=" "
fi
KILN_NUMBER_RUSTFLAGS+="-C panic=abort -C relocation-model=pic"

env RUSTFLAGS="$KILN_NUMBER_RUSTFLAGS" cargo rustc --locked \
	-p kiln-native-number-ffi --release --crate-type staticlib

KILN_NUMBER_TARGET_DIR="${CARGO_TARGET_DIR:-target}"
if [[ $KILN_NUMBER_TARGET_DIR != /* ]]; then
	KILN_NUMBER_TARGET_DIR="$(kiln_repo_root)/$KILN_NUMBER_TARGET_DIR"
fi
KILN_NUMBER_RELEASE_DIR="$KILN_NUMBER_TARGET_DIR/release"
if [[ -n ${CARGO_BUILD_TARGET:-} ]]; then
	KILN_NUMBER_RELEASE_DIR="$KILN_NUMBER_TARGET_DIR/$CARGO_BUILD_TARGET/release"
fi
KILN_NUMBER_ARCHIVE="$KILN_NUMBER_RELEASE_DIR/libkiln_native_number_ffi.a"
if [[ ! -f $KILN_NUMBER_ARCHIVE ]]; then
	printf 'number provider archive was not produced at %s\n' "$KILN_NUMBER_ARCHIVE" >&2
	exit 1
fi

KILN_NUMBER_CC="${CC:-cc}"
if ! command -v "$KILN_NUMBER_CC" >/dev/null 2>&1; then
	printf 'C driver %s is not available\n' "$KILN_NUMBER_CC" >&2
	exit 1
fi

KILN_NUMBER_TEMP="$(mktemp -d "${TMPDIR:-/tmp}/kiln-number-ffi.XXXXXX")"
kiln_number_cleanup() {
	find "$KILN_NUMBER_TEMP" -type f -delete
	rmdir "$KILN_NUMBER_TEMP"
}
trap kiln_number_cleanup EXIT

"$KILN_NUMBER_CC" -std=c11 -pedantic -Wall -Wextra -Werror \
	crates/kiln-native-number-ffi/tests/ffi_harness.c \
	"$KILN_NUMBER_ARCHIVE" -lm -o "$KILN_NUMBER_TEMP/probe"
"$KILN_NUMBER_TEMP/probe"
