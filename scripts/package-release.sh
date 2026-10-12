#!/usr/bin/env bash
# Build a kiln release: one archive per target holding the binary and everything `kiln build`
# needs at run time, so a machine with a C compiler and no Rust toolchain can compile programs.
#
#   scripts/package-release.sh [--target TRIPLE] [--variants all|core] OUTPUT_DIRECTORY
#
# Writes OUTPUT_DIRECTORY/kiln-<version>-<target>.tar.gz and a .sha256 beside it. Inside:
#
#   kiln-<version>-<target>/
#     bin/kiln                         the CLI, daemon and interpreter, one executable
#     lib/kiln/kiln-native-provider*.json, libkiln_native_provider_ffi*.a
#                                      every provider variant; `kiln build` picks the smallest
#                                      that covers the program (ADR 0062) and finds them here
#                                      through bin/../lib/kiln (number_provider.rs)
#     LICENSE, README.md, THIRD-PARTY-LICENSES.md
#
# Without the provider archives a native build falls back to compiling one from source, which
# needs cargo and this repository; that is the whole reason a release is more than the binary.
#
# `--variants core` packages only the variants nearly every program uses (realm, common, full,
# and full-gui on macOS) for a quicker local check; a published release is always `all`.
# The archive is smoke-tested before it is written: it is unpacked outside the repository and run
# with a PATH that has no cargo or rustc, so a release that silently needs the source tree fails
# here rather than on a user's machine.
# `mapfile` needs bash 4; macOS ships 3.2 as /bin/bash. Re-exec once under a newer one, the way
# scripts/native.sh does.
if [[ ${BASH_VERSINFO[0]} -lt 4 ]]; then
	for candidate in /opt/homebrew/bin/bash /usr/local/bin/bash "$(command -v bash)"; do
		# shellcheck disable=SC2016
		if [[ -x $candidate ]] && [[ $("$candidate" -c 'echo ${BASH_VERSINFO[0]}') -ge 4 ]]; then
			exec "$candidate" "${BASH_SOURCE[0]}" "$@"
		fi
	done
	echo "package-release.sh: needs bash 4 or newer; install one (brew install bash)" >&2
	exit 1
fi

set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
root=$(kiln_repo_root)
cd "$root"

target=""
variants=all
while [[ $# -gt 1 ]]; do
	case $1 in
	--target)
		target=$2
		shift 2
		;;
	--variants)
		variants=$2
		shift 2
		;;
	*) break ;;
	esac
done
if [[ $# -ne 1 || ! $variants =~ ^(all|core)$ ]]; then
	printf 'usage: %s [--target TRIPLE] [--variants all|core] OUTPUT_DIRECTORY\n' "$0" >&2
	exit 2
fi
output=$1
[[ $output == /* ]] || output="$PWD/$output"

host=$(rustc -vV | sed -n 's/^host: //p')
target=${target:-$host}
if [[ $target != "$host" ]]; then
	# The smoke test runs what it packaged, and the provider packager links a probe against the
	# archive; neither can happen for a foreign target. CI builds each target on its own runner.
	printf 'package-release: cross-packaging is not supported (host %s, target %s)\n' "$host" "$target" >&2
	exit 2
fi

version=$(sed -nE 's/^version = "([^"]+)"/\1/p' Cargo.toml | head -1)
commit=$(git rev-parse --short=12 HEAD)
if [[ -n $(git status --porcelain --untracked-files=no) && -z ${KILN_RELEASE_ALLOW_DIRTY:-} ]]; then
	printf 'package-release: the working tree has uncommitted changes; a release names a commit.\n' >&2
	printf 'Commit them, or set KILN_RELEASE_ALLOW_DIRTY=1 for a local trial.\n' >&2
	exit 1
fi
if [[ -n ${KILN_RELEASE_ALLOW_DIRTY:-} && -n $(git status --porcelain --untracked-files=no) ]]; then
	commit="$commit-dirty"
fi

name="kiln-$version-$target"
work=$(mktemp -d "${TMPDIR:-/tmp}/kiln-release.XXXXXX")
trap 'rm -rf "$work"' EXIT
stage="$work/$name"
mkdir -p "$stage/bin" "$stage/lib/kiln" "$output"

kiln_say "kiln $version ($commit) for $target"

kiln_say "building the CLI"
# Remap only the CLI build. The provider packager owns its exact Rust flags and compatibility
# identity, and deliberately clears this override. Encode arguments so checkout paths with
# spaces still form one rustc argument. Preserve caller flags (encoded flags take precedence).
cli_rustflags=$(python3 - "$root" "${CARGO_HOME:-$HOME/.cargo}" <<'PYFLAGS'
import os, shlex, sys
flags = (os.environ["CARGO_ENCODED_RUSTFLAGS"].split("\x1f")
         if os.environ.get("CARGO_ENCODED_RUSTFLAGS") else shlex.split(os.environ.get("RUSTFLAGS", "")))
flags += [f"--remap-path-prefix={sys.argv[1]}=/kiln", f"--remap-path-prefix={sys.argv[2]}=/cargo"]
print("\x1f".join(flags))
PYFLAGS
)
KILN_BUILD_COMMIT=$commit KILN_BUILD_TARGET=$target CARGO_ENCODED_RUSTFLAGS=$cli_rustflags \
	kiln_cargo build --release --locked -p kiln-cli --bin kiln
cp target/release/kiln "$stage/bin/kiln"
# The release profile keeps line tables (`debug = 1`) for local profiling; shipped, they are
# three quarters of the binary (409 MB -> 84 MB on x86_64 Linux). Strip the debug sections and
# keep the symbol table, so a panic's backtrace still names its functions.
case "$target" in
*-apple-darwin) strip -S "$stage/bin/kiln" ;;
*) strip --strip-debug "$stage/bin/kiln" ;;
esac

# Variant names come from the contract, so a variant added there is packaged without editing this.
contract_bin="$work/contract"
kiln_cargo build --release --locked -p kiln-native-provider-contract --bin kiln-native-provider-contract
cp target/release/kiln-native-provider-contract "$contract_bin"
mapfile -t all_variants < <("$contract_bin" contract |
	python3 -c 'import json,sys; [print(v["variant"]) for v in json.load(sys.stdin)["variants"]]')
case $(uname -s) in
Darwin) gui=1 ;;
*) gui=0 ;;
esac
selected=()
for variant in "${all_variants[@]}"; do
	# The GUI bridge is AppKit today (ADR 0074); a Linux release carries no GUI variant, and a
	# program importing kiln:gui there is refused by name rather than linked against nothing.
	if [[ $variant == gui || $variant == full-gui ]] && ((gui == 0)); then
		continue
	fi
	if [[ $variants == core && ! $variant =~ ^(realm|common|full|full-gui)$ ]]; then
		continue
	fi
	selected+=("$variant")
done

for variant in "${selected[@]}"; do
	kiln_say "packaging provider variant $variant"
	scripts/package-native-number-provider.sh --variant "$variant" "$stage/lib/kiln" >"$work/provider-$variant.log" 2>&1 || {
		cat "$work/provider-$variant.log" >&2
		exit 1
	}
done

cp LICENSE README.md SECURITY.md "$stage/"
kiln_say "collecting third-party licenses"
python3 scripts/third-party-licenses.py "$stage/THIRD-PARTY-LICENSES.md"

# ---- smoke test: the unpacked release, outside the repository, with no Rust toolchain --------
kiln_say "smoke-testing the release"
probe="$work/probe"
mkdir -p "$probe"
cat >"$probe/hello.ts" <<'TS'
const greet = (name: string): string => `hello, ${name}`;
console.log(greet("release"));
TS
cat >"$probe/regexp.ts" <<'TS'
const match = /(\d+)-(\d+)/.exec("range 12-34");
console.log(match ? Number(match[1]) + Number(match[2]) : -1);
TS
printf '{"name":"probe","private":true}\n' >"$probe/package.json"
bare_path=/usr/bin:/bin:/usr/sbin:/sbin
# TMPDIR is deliberately deep: it is where the daemon's socket would go, a one-shot command must
# not need one, and macOS caps a socket path at about 100 bytes. The daemon run below uses a short
# TMPDIR of its own, as a user's would be.
smoke_tmp=$work
smoke() {
	(cd "$probe" && env -i HOME="$probe" PATH="$bare_path" TMPDIR="$smoke_tmp" "$@")
}
if smoke sh -c 'command -v cargo || command -v rustc'; then
	printf 'package-release: the smoke-test PATH still reaches a Rust toolchain\n' >&2
	exit 1
fi
expect() {
	local want=$1
	shift
	local got
	got=$(smoke "$@" 2>&1) || {
		printf 'package-release: smoke test failed: %s\n%s\n' "$*" "$got" >&2
		exit 1
	}
	if [[ $got != *"$want"* ]]; then
		printf 'package-release: smoke test %s printed:\n%s\nexpected it to contain: %s\n' "$*" "$got" "$want" >&2
		exit 1
	fi
}
kiln="$stage/bin/kiln"
expect "kiln $version ($commit $target)" "$kiln" --version
expect "hello, release" "$kiln" --one-shot run hello.ts
expect "hello, release" "$kiln" --one-shot run hello.ts --native
expect "46" "$kiln" --one-shot run regexp.ts --native
smoke "$kiln" --one-shot build hello.ts --output hello >/dev/null
expect "hello, release" ./hello
# Installs link bin/kiln into a directory on PATH; the providers must still be found through the
# link's target, not beside the link (macOS's current_exe answers with the link's own path).
mkdir -p "$probe/linked-bin"
ln -s "$kiln" "$probe/linked-bin/kiln"
expect "46" "$probe/linked-bin/kiln" --one-shot run regexp.ts --native
# The resident daemon: `kiln run` without --one-shot starts it, and the same binary serves it.
smoke_tmp=$(mktemp -d /tmp/kiln-smoke.XXXXXX)
expect "hello, release" "$kiln" run hello.ts
smoke "$kiln" daemon stop >/dev/null 2>&1 || true
rm -rf "$smoke_tmp"
smoke_tmp=$work

# ---- archive ------------------------------------------------------------------------------
archive="$output/$name.tar.gz"
tar -C "$work" -czf "$archive.partial" "$name"
mv "$archive.partial" "$archive"
# sha256sum on Linux, shasum on macOS; both print "<digest>  <name>", the format SHA256SUMS and
# install.sh expect. A missing tool must fail the release, not leave an empty checksum behind.
if command -v sha256sum >/dev/null 2>&1; then
	(cd "$output" && sha256sum "$name.tar.gz" >"$name.tar.gz.sha256")
elif command -v shasum >/dev/null 2>&1; then
	(cd "$output" && shasum -a 256 "$name.tar.gz" >"$name.tar.gz.sha256")
else
	echo "package-release: needs sha256sum or shasum to checksum the archive" >&2
	exit 1
fi
if [[ ! -s "$output/$name.tar.gz.sha256" ]]; then
	echo "package-release: the checksum for $name.tar.gz is empty" >&2
	exit 1
fi
size=$(du -h "$archive" | cut -f1)
kiln_say "wrote $archive ($size, ${#selected[@]} provider variants)"
