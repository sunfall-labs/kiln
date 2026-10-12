#!/usr/bin/env bash
#
# Build and package the private Rust number provider for one explicit Rust target.
# The resulting directory is the unit kiln-native validates and ships: archive plus strict JSON
# manifest. This host packager deliberately does not claim a release target matrix; CI/release
# jobs choose CARGO_BUILD_TARGET and the deployment/libc baselines explicitly.
set -euo pipefail
# Contract metadata and source identity are provided by kiln-native-provider-contract.

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
source "$(dirname -- "${BASH_SOURCE[0]}")/provider-contract.sh"
cd "$(kiln_repo_root)"
KILN_PROVIDER_ROOT="$(kiln_repo_root)"
kiln_provider_contract_init "$KILN_PROVIDER_ROOT"
KILN_PROVIDER_PROFILE=$(kiln_provider_contract_field profile)
KILN_PROVIDER_RUSTFLAGS=$(kiln_provider_contract_field rustflags)
KILN_PROVIDER_SCHEMA=$(kiln_provider_contract_field schema)
KILN_PROVIDER_NAME=$(kiln_provider_contract_field provider)
KILN_PROVIDER_FORMAT=$(kiln_provider_contract_field format_version)
KILN_PROVIDER_PARSE=$(kiln_provider_contract_field parse_version)

# Every non-full schema-4 variant contains Realm plus either one engine or the historical common
# set. Omitted engines remain refusal stubs, keeping every package compatible with the fixed ABI.
KILN_PROVIDER_VARIANT=full
if [[ ${1:-} == "--slim" ]]; then
	KILN_PROVIDER_VARIANT=common
	shift
elif [[ ${1:-} == "--variant" ]]; then
	[[ $# -ge 2 ]] || { printf '%s\n' '--variant requires a name' >&2; exit 2; }
	KILN_PROVIDER_VARIANT=$2
	shift 2
fi
if [[ $# -ne 1 ]]; then
	printf 'usage: %s [--slim|--variant NAME] OUTPUT_DIRECTORY\n' "$0" >&2
	printf 'variants: %s\n' "$("$KILN_PROVIDER_CONTRACT_BIN" contract | python3 -c 'import json,sys; print(" ".join(v["variant"] for v in json.load(sys.stdin)["variants"]))')" >&2
	exit 2
fi

KILN_PROVIDER_PACKAGE_JSON=$(
	"$KILN_PROVIDER_CONTRACT_BIN" variant "$KILN_PROVIDER_VARIANT"
)
kiln_provider_package_field() {
	python3 -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]])' "$1" \
		<<<"$KILN_PROVIDER_PACKAGE_JSON"
}
KILN_PROVIDER_FEATURES=$(kiln_provider_package_field features)
KILN_PROVIDER_ARCHIVE_NAME=$(kiln_provider_package_field archive)
KILN_PROVIDER_MANIFEST_NAME=$(kiln_provider_package_field manifest)
# Whether this package carries the host GUI bridge (ADR 0074). Its archive then defines the
# `kiln_gui_*` exports too and needs the platform frameworks on every link line.
KILN_PROVIDER_HAS_GUI=$(
	python3 -c 'import json,sys; print("1" if "gui" in json.load(sys.stdin)["capabilities"] else "0")' \
		<<<"$KILN_PROVIDER_PACKAGE_JSON"
)

KILN_PROVIDER_OUTPUT=$1
if [[ $KILN_PROVIDER_OUTPUT != /* ]]; then
	KILN_PROVIDER_OUTPUT="$(kiln_repo_root)/$KILN_PROVIDER_OUTPUT"
fi

# mbx where it is installed (the shared cache and CPU budget, AGENTS.md); cargo elsewhere, which
# is what a release runner has.
if command -v mbx >/dev/null 2>&1; then
	KILN_PROVIDER_DEFAULT_CARGO=mbx
else
	KILN_PROVIDER_DEFAULT_CARGO=cargo
fi
KILN_PROVIDER_CARGO="${KILN_PROVIDER_CARGO:-${CARGO:-$KILN_PROVIDER_DEFAULT_CARGO}}"
# rustc's `--print native-static-libs` note, when the compiler actually runs. A cached or fresh
# build prints nothing, so this is a cross-check on the contract's own list below, never its source.
KILN_PROVIDER_RUSTC_STDERR="$(mktemp "${TMPDIR:-/tmp}/kiln-provider-rustc.XXXXXX")"
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

# Without `default-features = false` on the `kiln-runtime` edge in
# `crates/native/kiln-native-provider-ffi/Cargo.toml`, the common `--no-default-features` selection
# below would still unify `kiln-runtime/http` back on through that dependency's defaults
# and ship a full archive under a slim manifest.
# `${array[@]+"${array[@]}"}` below: an empty array is "unbound" to `set -u` under bash before
# 4.4, which is what `/bin/bash` is on macOS, and the full package is exactly the empty case.
if [[ $KILN_PROVIDER_VARIANT == full ]]; then
	KILN_PROVIDER_DEFAULT_FEATURES=()
else
	KILN_PROVIDER_DEFAULT_FEATURES=(--no-default-features)
fi

env -u CARGO_ENCODED_RUSTFLAGS -u CARGO_BUILD_RUSTFLAGS \
	RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER= \
	CARGO_BUILD_RUSTC_WRAPPER= CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER= \
	RUSTC="$KILN_PROVIDER_RUSTC" CARGO_BUILD_RUSTC="$KILN_PROVIDER_RUSTC" \
	RUSTFLAGS="$KILN_PROVIDER_RUSTFLAGS" "$KILN_PROVIDER_CARGO" rustc --locked \
	-p "$KILN_PROVIDER_NAME" --profile "$KILN_PROVIDER_PROFILE" \
	${KILN_PROVIDER_DEFAULT_FEATURES[@]+"${KILN_PROVIDER_DEFAULT_FEATURES[@]}"} \
	--features "$KILN_PROVIDER_FEATURES" \
	--target "$KILN_PROVIDER_TARGET" --crate-type staticlib -- --print native-static-libs \
	2> >(tee "$KILN_PROVIDER_RUSTC_STDERR" >&2)

KILN_PROVIDER_TARGET_DIR="${CARGO_TARGET_DIR:-target}"
if [[ $KILN_PROVIDER_TARGET_DIR != /* ]]; then
	KILN_PROVIDER_TARGET_DIR="$(kiln_repo_root)/$KILN_PROVIDER_TARGET_DIR"
fi
KILN_PROVIDER_BUILT_ARCHIVE="$KILN_PROVIDER_TARGET_DIR/$KILN_PROVIDER_TARGET/$KILN_PROVIDER_PROFILE/lib${KILN_PROVIDER_NAME//-/_}.a"
if [[ ! -f $KILN_PROVIDER_BUILT_ARCHIVE ]]; then
	printf 'number provider archive was not produced at %s\n' \
		"$KILN_PROVIDER_BUILT_ARCHIVE" >&2
	exit 1
fi

mkdir -p "$KILN_PROVIDER_OUTPUT"
KILN_PROVIDER_ARCHIVE="$KILN_PROVIDER_OUTPUT/$KILN_PROVIDER_ARCHIVE_NAME"
KILN_PROVIDER_MANIFEST="$KILN_PROVIDER_OUTPUT/$KILN_PROVIDER_MANIFEST_NAME"
KILN_PROVIDER_ARCHIVE_TEMP="$KILN_PROVIDER_OUTPUT/.$KILN_PROVIDER_ARCHIVE_NAME.$$.tmp"
KILN_PROVIDER_MANIFEST_TEMP="$KILN_PROVIDER_OUTPUT/.$KILN_PROVIDER_MANIFEST_NAME.$$.tmp"
KILN_PROVIDER_PROBE_DIR=""
kiln_provider_cleanup() {
	rm -f "$KILN_PROVIDER_ARCHIVE_TEMP" "$KILN_PROVIDER_MANIFEST_TEMP" "$KILN_PROVIDER_RUSTC_STDERR"
	[[ -z $KILN_PROVIDER_PROBE_DIR ]] || rm -rf "$KILN_PROVIDER_PROBE_DIR"
}
trap kiln_provider_cleanup EXIT

# Keep the shipped manifest tied to the ABI the resolver validates. This is intentionally read from
# the Rust source rather than duplicated as a second version constant in the shell packager.
KILN_PROVIDER_ABI_VERSION=$(
	"$KILN_PROVIDER_CONTRACT_BIN" abi
)
if [[ ! $KILN_PROVIDER_ABI_VERSION =~ ^[0-9]+$ ]]; then
	printf 'could not determine number-provider ABI version\n' >&2
	exit 1
fi

# The export list likewise, and for a sharper reason: the resolver rejects an archive whose
# manifest does not name *exactly* the symbols it requires. This intentionally bounded extraction
# reads the package's own list -- the registry entries plus, for a package that carries the host
# GUI bridge, the bridge's `kiln_gui_*` operations -- rather than maintaining another list here.
KILN_PROVIDER_EXPORTS=$(
	python3 -c 'import json,sys; print(" ".join(json.load(sys.stdin)["exports"]))' \
		<<<"$KILN_PROVIDER_PACKAGE_JSON" |
		awk '{for (i=1; i<=NF; i++) printf "%s\"%s\"", (i==1 ? "" : ","), $i}'
)
if [[ -z $KILN_PROVIDER_EXPORTS ]]; then
	printf 'could not determine the number-provider export list\n' >&2
	exit 1
fi

# Prove the archive actually *defines* every symbol the manifest is about to promise.
#
# Everything upstream of here is derived from the same macro registry, so the manifest and the
# resolver's `PROVIDER_EXPORTS` agree with each other by construction and neither of them has ever
# looked at the archive. `regexp`, `bigint`, `temporal` and `json` are off by default, so an archive built
# without those features is missing 20 of the 69 exports and still passes every check the compiler
# makes -- the failure surfaces much later as `Undefined symbols for architecture arm64` from the
# linker of whatever program happened to be built next, naming a symbol rather than a cause.
KILN_PROVIDER_NM="${NM:-nm}"
if command -v "$KILN_PROVIDER_NM" >/dev/null 2>&1; then
	# `nm` exits non-zero merely because some archive members carry no symbols at all, which is
	# ordinary. Take its status separately from its output, or `set -e` aborts this script with no
	# message whatsoever -- which is exactly what the first draft of this check did.
	if [[ $KILN_PROVIDER_HAS_GUI == 1 ]]; then
		KILN_PROVIDER_NM_OUTPUT=$("$KILN_PROVIDER_NM" "$KILN_PROVIDER_BUILT_ARCHIVE" 2>/dev/null) ||
			true
	else
		KILN_PROVIDER_NM_OUTPUT=$("$KILN_PROVIDER_NM" -g "$KILN_PROVIDER_BUILT_ARCHIVE" 2>/dev/null) ||
			true
	fi
	# A defined symbol has a type letter; `U` is undefined and `-g` keeps only external ones.
	# Mach-O prefixes C symbols with an underscore and ELF does not, so accept either spelling.
	# The bridge's operations are reached through the archive's own table, not the linker, so
	# whole-archive LTO leaves them as local (`t`) definitions: accept any defined symbol.
	KILN_PROVIDER_DEFINED=$(
		printf '%s\n' "$KILN_PROVIDER_NM_OUTPUT" |
			awk '$2 ~ /^[A-Za-z]$/ && $2 != "U" { name = $3; sub(/^_/, "", name); print name }' |
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
			'Most often the archive was built without the requested features. Full builds every engine;' \
			'all other variants use --no-default-features with the exact contract feature set. Check that' \
			'the refusal stubs still cover every group in' \
			'provider_abi_registry! in crates/semantics/kiln-native-provider-contract/src/abi.rs.' >&2
		exit 1
	fi
else
	printf 'warning: %s is not available, so the archive symbol table was not verified\n' \
		"$KILN_PROVIDER_NM" >&2
fi

# What a program linking this archive must add to its link line (ADR 0074): the contract's list
# for this target, empty unless the package carries the host GUI bridge. Two proofs guard it.
#
# First, when rustc ran and printed its `native-static-libs` note, every library it names that the
# C runtime does not already link must be in the contract's list; a dependency that starts linking
# a new framework then fails here instead of at some program's link. Second, link a probe that
# takes the address of every bridge export -- which is what pulls the bridge's objects out of the
# archive, since a static link brings in only what is referenced -- so a missing framework fails
# now with its symbol named, with the fix in this file.
KILN_PROVIDER_LINK_ARGUMENTS=""
if [[ $KILN_PROVIDER_HAS_GUI == 1 ]]; then
	KILN_PROVIDER_LINK_ARGUMENTS=$("$KILN_PROVIDER_CONTRACT_BIN" gui-link-arguments "$KILN_PROVIDER_TARGET")
	KILN_PROVIDER_NOTE=$(sed -n 's/^note: native-static-libs: //p' "$KILN_PROVIDER_RUSTC_STDERR" | tail -1)
	if [[ -n $KILN_PROVIDER_NOTE ]]; then
		KILN_PROVIDER_NOTE_MISSING=""
		KILN_PROVIDER_NOTE_WORDS=($KILN_PROVIDER_NOTE)
		for ((i = 0; i < ${#KILN_PROVIDER_NOTE_WORDS[@]}; i++)); do
			KILN_PROVIDER_WORD=${KILN_PROVIDER_NOTE_WORDS[i]}
			if [[ $KILN_PROVIDER_WORD == -framework ]]; then
				((i++))
				KILN_PROVIDER_ITEM="-framework ${KILN_PROVIDER_NOTE_WORDS[i]}"
			else
				KILN_PROVIDER_ITEM=$KILN_PROVIDER_WORD
			fi
			case "$KILN_PROVIDER_ITEM" in
				-lSystem | -lc | -lm | -lpthread | -ldl | -lgcc_s | -lutil | -lrt | -lgcc) continue ;;
			esac
			[[ " $KILN_PROVIDER_LINK_ARGUMENTS " == *" $KILN_PROVIDER_ITEM "* ]] ||
				KILN_PROVIDER_NOTE_MISSING+=" $KILN_PROVIDER_ITEM"
		done
		if [[ -n $KILN_PROVIDER_NOTE_MISSING ]]; then
			printf 'rustc says this archive links native libraries the contract does not list:%s\n' \
				"$KILN_PROVIDER_NOTE_MISSING" >&2
			printf 'add them to `link_arguments` in crates/semantics/kiln-native-provider-contract/src/gui.rs\n' >&2
			exit 1
		fi
	fi
	KILN_PROVIDER_PROBE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/kiln-provider-probe.XXXXXX")"
	KILN_PROVIDER_PROBE_CC="${CC:-cc}"
	if command -v "$KILN_PROVIDER_PROBE_CC" >/dev/null 2>&1; then
		{
			printf '#include <stddef.h>\n'
			for KILN_PROVIDER_EXPORT in ${KILN_PROVIDER_EXPORTS//[\",]/ }; do
				case "$KILN_PROVIDER_EXPORT" in
					kiln_gui_*) printf 'void %s(void);\n' "$KILN_PROVIDER_EXPORT" ;;
				esac
			done
			printf 'int kiln_ffi_last_error(const unsigned char **bytes, unsigned long long *len);\n'
			printf 'void *kiln_provider_probe_table[] = {\n'
			for KILN_PROVIDER_EXPORT in ${KILN_PROVIDER_EXPORTS//[\",]/ }; do
				case "$KILN_PROVIDER_EXPORT" in
					kiln_gui_*) printf '    (void *)%s,\n' "$KILN_PROVIDER_EXPORT" ;;
				esac
			done
			printf '    (void *)kiln_ffi_last_error,\n};\n'
			printf 'int main(void) { return kiln_provider_probe_table[0] == NULL; }\n'
		} >"$KILN_PROVIDER_PROBE_DIR/probe.c"
		# shellcheck disable=SC2086 # the contract's arguments are a space-separated flag list
		if ! "$KILN_PROVIDER_PROBE_CC" "$KILN_PROVIDER_PROBE_DIR/probe.c" "$KILN_PROVIDER_BUILT_ARCHIVE" \
			-lm $KILN_PROVIDER_LINK_ARGUMENTS -o "$KILN_PROVIDER_PROBE_DIR/probe" 2>"$KILN_PROVIDER_PROBE_DIR/link.log"; then
			printf 'the %s archive does not link with the contract'"'"'s link arguments (%s):\n' \
				"$KILN_PROVIDER_VARIANT" "$KILN_PROVIDER_LINK_ARGUMENTS" >&2
			cat "$KILN_PROVIDER_PROBE_DIR/link.log" >&2
			printf 'fix `link_arguments` in crates/semantics/kiln-native-provider-contract/src/gui.rs\n' >&2
			exit 1
		fi
	else
		printf 'warning: %s is not available, so the archive link arguments were not verified\n' \
			"$KILN_PROVIDER_PROBE_CC" >&2
	fi
fi

cp "$KILN_PROVIDER_BUILT_ARCHIVE" "$KILN_PROVIDER_ARCHIVE_TEMP"
KILN_PROVIDER_ARCHIVE_DIGEST=$(kiln_provider_sha256_file "$KILN_PROVIDER_ARCHIVE_TEMP")
KILN_PROVIDER_RUSTC_DIGEST=$("$KILN_PROVIDER_RUSTC" -vV | kiln_provider_sha256_stream)
KILN_PROVIDER_LISTED="$(mktemp)"
KILN_PROVIDER_SOURCE_DIGEST=$(
	"$KILN_PROVIDER_CONTRACT_BIN" source-digest "$(kiln_repo_root)"
)
while IFS= read -r KILN_PROVIDER_SOURCE || [[ -n $KILN_PROVIDER_SOURCE ]]; do
	[[ -z $KILN_PROVIDER_SOURCE ]] || printf '%s\n' "$KILN_PROVIDER_SOURCE" >>"$KILN_PROVIDER_LISTED"
done < crates/semantics/kiln-native-provider-contract/provider-sources.txt

# The manifest is intentionally explicit, and an explicit list of inputs has one failure mode: a
# file that belongs in it and is not. That file's contents then cannot change the digest,
# so a stale archive is reused and the native link fails with `undefined reference` -- or worse,
# links something whose behaviour moved. `kiln-node-path` was exactly that on 2026-09-02.
#
# rustc already knows the answer and writes it down, so ask it rather than a reviewer. Everything
# the compiler read from this workspace must be named in the list; anything else is a file whose
# edits the digest cannot see. The compiler and both scripts consume the same manifest; `kiln-cli`'s
# `script_hygiene` test keeps that shared boundary present. This is the completeness check.
KILN_PROVIDER_DEP_INFO="$KILN_PROVIDER_TARGET_DIR/$KILN_PROVIDER_TARGET/$KILN_PROVIDER_PROFILE/lib${KILN_PROVIDER_NAME//-/_}.d"
if [[ -f $KILN_PROVIDER_DEP_INFO ]]; then
	KILN_PROVIDER_ROOT="$(kiln_repo_root)"
	KILN_PROVIDER_UNLISTED=$(
		tr ' ' '\n' <"$KILN_PROVIDER_DEP_INFO" |
			sed -n "s|^$KILN_PROVIDER_ROOT/||p" |
			grep '\.rs$' |
			sort -u |
			while IFS= read -r KILN_PROVIDER_READ; do
				grep -qxF "$KILN_PROVIDER_READ" "$KILN_PROVIDER_LISTED" ||
					printf '  %s\n' "$KILN_PROVIDER_READ"
			done
	)
	if [[ -n $KILN_PROVIDER_UNLISTED ]]; then
		printf 'the provider archive is compiled from workspace sources the identity list does not name:\n%s\n' \
			"$KILN_PROVIDER_UNLISTED" >&2
		printf 'add them to crates/semantics/kiln-native-provider-contract/provider-sources.txt, or their edits cannot invalidate the archive.\n' >&2
		exit 1
	fi
fi

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

"$KILN_PROVIDER_CONTRACT_BIN" manifest \
	"$KILN_PROVIDER_TARGET" "sha256:$KILN_PROVIDER_RUSTC_DIGEST" \
	"$KILN_PROVIDER_SOURCE_DIGEST" "$KILN_PROVIDER_ARCHIVE_DIGEST" \
	"$KILN_PROVIDER_DEPLOYMENT" "$KILN_PROVIDER_LIBC" "$KILN_PROVIDER_VARIANT" \
	"$KILN_PROVIDER_LINK_ARGUMENTS" \
	>"$KILN_PROVIDER_MANIFEST_TEMP"

mv -f "$KILN_PROVIDER_ARCHIVE_TEMP" "$KILN_PROVIDER_ARCHIVE"
mv -f "$KILN_PROVIDER_MANIFEST_TEMP" "$KILN_PROVIDER_MANIFEST"
printf '%s\n' "$KILN_PROVIDER_MANIFEST"
