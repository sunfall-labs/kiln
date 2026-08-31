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
	-p kiln-native-number-ffi --release --features regexp,bigint \
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
	sed -n 's/^pub(crate) const PROVIDER_ABI_VERSION: u32 = \([0-9][0-9]*\);$/\1/p' \
		crates/kiln-native/src/number_provider.rs
)
if [[ ! $KILN_PROVIDER_ABI_VERSION =~ ^[0-9]+$ ]]; then
	printf 'could not determine number-provider ABI version\n' >&2
	exit 1
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
		crates/kiln-runtime/Cargo.toml \
		crates/kiln-runtime/src/lib.rs; do
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
	'  "exports": ["kiln_realm_create", "kiln_realm_enter", "kiln_realm_is_active", "kiln_realm_now", "kiln_realm_queue_microtask", "kiln_realm_set_timeout", "kiln_realm_set_interval", "kiln_realm_clear_timer", "kiln_realm_drive", "kiln_realm_destroy", "kiln_realm_read_file", "kiln_realm_start_host_op", "kiln_realm_start_active_host_op", "kiln_realm_cancel_request", "kiln_realm_cancel_active_request", "kiln_http1_decoder_create", "kiln_http1_decoder_free", "kiln_http1_decoder_push", "kiln_http1_response_encode", "kiln_http1_response_decoder_create", "kiln_http1_response_decoder_free", "kiln_http1_response_decoder_push", "kiln_http1_response_events_len", "kiln_http1_response_events_get", "kiln_http1_response_events_free", "kiln_number_to_js_string", "kiln_string_to_number", "kiln_regexp_compile", "kiln_regexp_free", "kiln_regexp_search", "kiln_regexp_group_count", "kiln_regexp_group_name", "kiln_bigint_parse", "kiln_bigint_from_double", "kiln_bigint_free", "kiln_bigint_to_double", "kiln_bigint_to_string", "kiln_bigint_binary", "kiln_bigint_negate", "kiln_bigint_compare", "kiln_bigint_is_zero"]' \
	'}' >"$KILN_PROVIDER_MANIFEST_TEMP"

mv -f "$KILN_PROVIDER_ARCHIVE_TEMP" "$KILN_PROVIDER_ARCHIVE"
mv -f "$KILN_PROVIDER_MANIFEST_TEMP" "$KILN_PROVIDER_MANIFEST"
printf '%s\n' "$KILN_PROVIDER_MANIFEST"
