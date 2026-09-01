#!/usr/bin/env bash
#
# Build and package the private Rust number provider for one explicit Rust target.
# The resulting directory is the unit kiln-native validates and ships: archive plus strict JSON
# manifest. This host packager deliberately does not claim a release target matrix; CI/release
# jobs choose CARGO_BUILD_TARGET and the deployment/libc baselines explicitly.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

if [[ $# -ne 1 ]]; then
	printf 'usage: %s OUTPUT_DIRECTORY\n' "$0" >&2
	exit 2
fi

KILN_PROVIDER_OUTPUT=$1
if [[ $KILN_PROVIDER_OUTPUT != /* ]]; then
	KILN_PROVIDER_OUTPUT="$(kiln_repo_root)/$KILN_PROVIDER_OUTPUT"
fi

KILN_PROVIDER_CARGO="${CARGO:-cargo}"
KILN_PROVIDER_RUSTC="${RUSTC:-rustc}"
if ! command -v "$KILN_PROVIDER_CARGO" >/dev/null 2>&1; then
	printf 'Cargo executable %s is not available\n' "$KILN_PROVIDER_CARGO" >&2
	exit 1
fi
if ! command -v "$KILN_PROVIDER_RUSTC" >/dev/null 2>&1; then
	printf 'Rust compiler %s is not available\n' "$KILN_PROVIDER_RUSTC" >&2
	exit 1
fi

KILN_PROVIDER_TARGET="${CARGO_BUILD_TARGET:-}"
if [[ -z $KILN_PROVIDER_TARGET ]]; then
	KILN_PROVIDER_TARGET=$("$KILN_PROVIDER_RUSTC" -vV | sed -n 's/^host: //p')
fi
if [[ -z $KILN_PROVIDER_TARGET ]]; then
	printf 'rustc did not report a host target\n' >&2
	exit 1
fi
case "$KILN_PROVIDER_TARGET" in
	*-apple-darwin | *-linux-gnu | *-linux-musl) ;;
	*)
		printf 'number-provider packaging does not yet support target %s\n' \
			"$KILN_PROVIDER_TARGET" >&2
		exit 1
		;;
esac

kiln_provider_sha256_file() {
	if command -v shasum >/dev/null 2>&1; then
		shasum -a 256 "$1" | awk '{print $1}'
	elif command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | awk '{print $1}'
	else
		printf 'neither shasum nor sha256sum is available\n' >&2
		return 1
	fi
}

kiln_provider_sha256_stream() {
	if command -v shasum >/dev/null 2>&1; then
		shasum -a 256 | awk '{print $1}'
	elif command -v sha256sum >/dev/null 2>&1; then
		sha256sum | awk '{print $1}'
	else
		printf 'neither shasum nor sha256sum is available\n' >&2
		return 1
	fi
}

# Keep this policy in lockstep with `PROVIDER_RUSTFLAGS` in `kiln-native::number_provider`. RUSTFLAGS
# outranks Cargo config; removing the encoded form prevents a caller's host-only flags from silently
# changing that deployment contract. The unwind policy is required because provider entries catch
# Rust panics before returning across the private C ABI.
KILN_PROVIDER_RUSTFLAGS="-C panic=unwind -C relocation-model=pic -C target-cpu=generic -C debuginfo=0"

env -u CARGO_ENCODED_RUSTFLAGS -u CARGO_BUILD_RUSTFLAGS \
	RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER= \
	CARGO_BUILD_RUSTC_WRAPPER= CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER= \
	RUSTC="$KILN_PROVIDER_RUSTC" CARGO_BUILD_RUSTC="$KILN_PROVIDER_RUSTC" \
	RUSTFLAGS="$KILN_PROVIDER_RUSTFLAGS" "$KILN_PROVIDER_CARGO" rustc --locked \
	-p kiln-native-number-ffi --release --features regexp,bigint,temporal \
	--target "$KILN_PROVIDER_TARGET" --crate-type staticlib

KILN_PROVIDER_TARGET_DIR="${CARGO_TARGET_DIR:-target}"
if [[ $KILN_PROVIDER_TARGET_DIR != /* ]]; then
	KILN_PROVIDER_TARGET_DIR="$(kiln_repo_root)/$KILN_PROVIDER_TARGET_DIR"
fi
KILN_PROVIDER_BUILT_ARCHIVE="$KILN_PROVIDER_TARGET_DIR/$KILN_PROVIDER_TARGET/release/libkiln_native_number_ffi.a"
if [[ ! -f $KILN_PROVIDER_BUILT_ARCHIVE ]]; then
	printf 'number provider archive was not produced at %s\n' \
		"$KILN_PROVIDER_BUILT_ARCHIVE" >&2
	exit 1
fi

mkdir -p "$KILN_PROVIDER_OUTPUT"
KILN_PROVIDER_ARCHIVE_NAME=libkiln_native_number_ffi.a
KILN_PROVIDER_MANIFEST_NAME=kiln-native-number-provider.json
KILN_PROVIDER_ARCHIVE="$KILN_PROVIDER_OUTPUT/$KILN_PROVIDER_ARCHIVE_NAME"
KILN_PROVIDER_MANIFEST="$KILN_PROVIDER_OUTPUT/$KILN_PROVIDER_MANIFEST_NAME"
KILN_PROVIDER_ARCHIVE_TEMP="$KILN_PROVIDER_OUTPUT/.$KILN_PROVIDER_ARCHIVE_NAME.$$.tmp"
KILN_PROVIDER_MANIFEST_TEMP="$KILN_PROVIDER_OUTPUT/.$KILN_PROVIDER_MANIFEST_NAME.$$.tmp"
kiln_provider_cleanup() {
	rm -f "$KILN_PROVIDER_ARCHIVE_TEMP" "$KILN_PROVIDER_MANIFEST_TEMP"
}
trap kiln_provider_cleanup EXIT

# Keep the shipped manifest tied to the ABI the resolver validates. This is intentionally read from
# the Rust source rather than duplicated as a second version constant in the shell packager.
KILN_PROVIDER_ABI_VERSION=$(
	sed -n 's/^pub const PROVIDER_ABI_VERSION: u32 = \([0-9][0-9]*\);$/\1/p' \
		crates/kiln-native-number-abi/src/lib.rs
)
if [[ ! $KILN_PROVIDER_ABI_VERSION =~ ^[0-9]+$ ]]; then
	printf 'could not determine number-provider ABI version\n' >&2
	exit 1
fi

# The export list likewise, and for a sharper reason: the resolver rejects an archive whose
# manifest does not name *exactly* the symbols it requires. This intentionally bounded extraction
# reads the registry entries, rather than maintaining another list in the packager.
KILN_PROVIDER_EXPORTS=$(
	sed -n '/^macro_rules! provider_abi_registry/,/^}/p' crates/kiln-native-number-abi/src/lib.rs |
		sed -n 's/.*\(kiln_[A-Za-z0-9_]*\).*/"\1"/p' |
		paste -sd, -
)
if [[ -z $KILN_PROVIDER_EXPORTS ]]; then
	printf 'could not determine the number-provider export list\n' >&2
	exit 1
fi

# Prove the archive actually *defines* every symbol the manifest is about to promise.
#
# Everything upstream of here is derived from the same macro registry, so the manifest and the
# resolver's `PROVIDER_EXPORTS` agree with each other by construction and neither of them has ever
# looked at the archive. `regexp`, `bigint` and `temporal` are off by default, so an archive built
# without those features is missing 20 of the 65 exports and still passes every check the compiler
# makes -- the failure surfaces much later as `Undefined symbols for architecture arm64` from the
# linker of whatever program happened to be built next, naming a symbol rather than a cause.
KILN_PROVIDER_NM="${NM:-nm}"
if command -v "$KILN_PROVIDER_NM" >/dev/null 2>&1; then
	# `nm` exits non-zero merely because some archive members carry no symbols at all, which is
	# ordinary. Take its status separately from its output, or `set -e` aborts this script with no
	# message whatsoever -- which is exactly what the first draft of this check did.
	KILN_PROVIDER_NM_OUTPUT=$("$KILN_PROVIDER_NM" -g "$KILN_PROVIDER_BUILT_ARCHIVE" 2>/dev/null) ||
		true
	# A defined symbol has a type letter; `U` is undefined and `-g` keeps only external ones.
	# Mach-O prefixes C symbols with an underscore and ELF does not, so accept either spelling.
	KILN_PROVIDER_DEFINED=$(
		printf '%s\n' "$KILN_PROVIDER_NM_OUTPUT" |
			awk '$2 ~ /^[A-TV-Z]$/ { name = $3; sub(/^_/, "", name); print name }' |
			sort -u
	)
	if [[ -z $KILN_PROVIDER_DEFINED ]]; then
		printf '%s read no symbols at all from %s, so the export check could not run\n' \
			"$KILN_PROVIDER_NM" "$KILN_PROVIDER_BUILT_ARCHIVE" >&2
		exit 1
	fi
	# Membership is tested with bash pattern matching rather than a `grep -q` per export: `grep -q`
	# exits at the first match and closes the pipe under it, so the writing `printf` reports
	# "broken pipe" for most of the 65 lookups and buries the real message in noise.
	KILN_PROVIDER_DEFINED_LINES=$'\n'$KILN_PROVIDER_DEFINED$'\n'
	KILN_PROVIDER_MISSING=""
	for KILN_PROVIDER_EXPORT in ${KILN_PROVIDER_EXPORTS//[\",]/ }; do
		if [[ $KILN_PROVIDER_DEFINED_LINES != *$'\n'"$KILN_PROVIDER_EXPORT"$'\n'* ]]; then
			KILN_PROVIDER_MISSING+=" $KILN_PROVIDER_EXPORT"
		fi
	done
	if [[ -n $KILN_PROVIDER_MISSING ]]; then
		printf 'the built provider archive does not define %d of its declared exports:\n' \
			"$(printf '%s' "$KILN_PROVIDER_MISSING" | wc -w | tr -d '[:space:]')" >&2
		for KILN_PROVIDER_EXPORT in $KILN_PROVIDER_MISSING; do
			printf '  %s\n' "$KILN_PROVIDER_EXPORT" >&2
		done
		printf '\n%s\n%s\n' \
			'Packaging it would ship a manifest promising symbols the archive lacks, and the' \
			'failure would resurface as "Undefined symbols" from an unrelated native link.' >&2
		printf '%s\n' \
			'Most often the archive was built without the optional features. This script builds' \
			'with --features regexp,bigint,temporal; check that those still cover every group in' \
			'provider_abi_registry! in crates/kiln-native-number-abi/src/lib.rs.' >&2
		exit 1
	fi
else
	printf 'warning: %s is not available, so the archive symbol table was not verified\n' \
		"$KILN_PROVIDER_NM" >&2
fi

cp "$KILN_PROVIDER_BUILT_ARCHIVE" "$KILN_PROVIDER_ARCHIVE_TEMP"
KILN_PROVIDER_ARCHIVE_DIGEST=$(kiln_provider_sha256_file "$KILN_PROVIDER_ARCHIVE_TEMP")
KILN_PROVIDER_RUSTC_DIGEST=$("$KILN_PROVIDER_RUSTC" -vV | kiln_provider_sha256_stream)
KILN_PROVIDER_SOURCE_DIGEST=$(
	for KILN_PROVIDER_SOURCE in \
		Cargo.lock \
		crates/kiln-number-core/Cargo.toml \
		crates/kiln-number-core/src/lib.rs \
		crates/kiln-native-number-ffi/Cargo.toml \
		crates/kiln-native-number-ffi/src/lib.rs \
		crates/kiln-native-number-abi/Cargo.toml \
		crates/kiln-native-number-abi/src/lib.rs \
		crates/kiln-temporal/Cargo.toml \
		crates/kiln-temporal/src/lib.rs \
		crates/kiln-runtime/Cargo.toml \
		crates/kiln-runtime/src/http_body.rs \
		crates/kiln-runtime/src/http_transport.rs \
		crates/kiln-runtime/src/hyper_http.rs \
		crates/kiln-runtime/src/lib.rs \
		crates/kiln-runtime/src/node_http.rs \
		crates/kiln-runtime/src/node_net.rs \
		crates/kiln-runtime/src/node_stream.rs; do
		KILN_PROVIDER_SIZE=$(wc -c <"$KILN_PROVIDER_SOURCE" | tr -d '[:space:]')
		printf '%s\0%s\0' "$KILN_PROVIDER_SOURCE" "$KILN_PROVIDER_SIZE"
		dd if="$KILN_PROVIDER_SOURCE" bs=65536 2>/dev/null
	done | kiln_provider_sha256_stream
)

KILN_PROVIDER_DEPLOYMENT="${MACOSX_DEPLOYMENT_TARGET:-}"
KILN_PROVIDER_LIBC="${KILN_LIBC_BASELINE:-}"
for KILN_PROVIDER_FIELD in \
	"$KILN_PROVIDER_TARGET" \
	"$KILN_PROVIDER_RUSTC_DIGEST" \
	"$KILN_PROVIDER_SOURCE_DIGEST" \
	"$KILN_PROVIDER_ARCHIVE_DIGEST" \
	"$KILN_PROVIDER_DEPLOYMENT" \
	"$KILN_PROVIDER_LIBC"; do
	if [[ ! $KILN_PROVIDER_FIELD =~ ^[A-Za-z0-9._:+/=\-]*$ ]]; then
		printf 'number-provider manifest field contains unsupported characters: %s\n' \
			"$KILN_PROVIDER_FIELD" >&2
		exit 1
	fi
done

printf '%s\n' \
	'{' \
	'  "schema": 1,' \
	'  "provider": "kiln-native-number-ffi",' \
	"  \"provider_abi\": $KILN_PROVIDER_ABI_VERSION," \
	'  "format_version": 1,' \
	'  "parse_version": 1,' \
	"  \"rust_target\": \"$KILN_PROVIDER_TARGET\"," \
	"  \"rustc_identity\": \"sha256:$KILN_PROVIDER_RUSTC_DIGEST\"," \
	"  \"source_sha256\": \"$KILN_PROVIDER_SOURCE_DIGEST\"," \
	"  \"archive_sha256\": \"$KILN_PROVIDER_ARCHIVE_DIGEST\"," \
	"  \"archive\": \"$KILN_PROVIDER_ARCHIVE_NAME\"," \
	'  "linkage": "static",' \
	'  "pic": true,' \
	'  "profile": "release",' \
	'  "cpu_features": "generic",' \
	"  \"deployment\": \"$KILN_PROVIDER_DEPLOYMENT\"," \
	"  \"libc_baseline\": \"$KILN_PROVIDER_LIBC\"," \
	"  \"exports\": [$KILN_PROVIDER_EXPORTS]" \
	'}' >"$KILN_PROVIDER_MANIFEST_TEMP"

mv -f "$KILN_PROVIDER_ARCHIVE_TEMP" "$KILN_PROVIDER_ARCHIVE"
mv -f "$KILN_PROVIDER_MANIFEST_TEMP" "$KILN_PROVIDER_MANIFEST"
printf '%s\n' "$KILN_PROVIDER_MANIFEST"
