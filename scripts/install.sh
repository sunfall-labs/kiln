#!/bin/sh
# Install kiln from a GitHub release.
#
#   curl -fsSL https://github.com/sunfall-labs/kiln/releases/latest/download/install.sh | sh
#
# Environment:
#   KILN_VERSION      a version such as 0.1.0 (default: the latest release)
#   KILN_INSTALL_DIR  where releases are unpacked (default: ~/.kiln)
#   KILN_BIN_DIR      where the `kiln` link goes (default: ~/.local/bin)
#   GITHUB_TOKEN      read access, needed while the repository is private
#
# A release is unpacked whole into $KILN_INSTALL_DIR/<version>/, because bin/kiln finds its native
# provider archives at bin/../lib/kiln; the link in $KILN_BIN_DIR points into it. Every download
# is checked against the release's SHA256SUMS before anything is unpacked. POSIX sh, so it runs
# under whatever `sh` the machine has.
set -eu

repo=sunfall-labs/kiln
install_dir=${KILN_INSTALL_DIR:-$HOME/.kiln}
bin_dir=${KILN_BIN_DIR:-$HOME/.local/bin}

say() { printf 'kiln-install: %s\n' "$*" >&2; }
die() {
	say "$*"
	exit 1
}

case "$(uname -s)-$(uname -m)" in
Darwin-arm64) target=aarch64-apple-darwin ;;
Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-gnu ;;
*) die "no release is built for $(uname -s) $(uname -m); build from source (see README.md)" ;;
esac

command -v curl >/dev/null 2>&1 || die "needs curl"
command -v tar >/dev/null 2>&1 || die "needs tar"
if command -v sha256sum >/dev/null 2>&1; then
	sha() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
	sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
	die "needs sha256sum or shasum to verify the download"
fi

fetch() {
	# $1 URL, $2 output file, $3 Accept header
	if [ -n "${GITHUB_TOKEN:-}" ]; then
		curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" -H "Accept: $3" -o "$2" "$1"
	else
		curl -fsSL -H "Accept: $3" -o "$2" "$1"
	fi
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM

if [ -n "${KILN_VERSION:-}" ]; then
	release_url="https://api.github.com/repos/$repo/releases/tags/v${KILN_VERSION#v}"
else
	release_url="https://api.github.com/repos/$repo/releases/latest"
fi
fetch "$release_url" "$work/release.json" application/vnd.github+json ||
	die "could not read the release from $release_url (private repository? set GITHUB_TOKEN)"

# Asset API URLs work for private repositories with a token, where browser URLs do not. The JSON
# is read with sed rather than jq, which is not on every machine.
tag=$(sed -n 's/^  "tag_name": "\(.*\)",$/\1/p' "$work/release.json" | head -1)
[ -n "$tag" ] || die "the release has no tag name"
version=${tag#v}
archive="kiln-$version-$target.tar.gz"
asset_url() {
	# Each asset object lists "url" before "name"; pair them up and pick the one named $1.
	tr -d '\n' <"$work/release.json" | tr '{' '\n' |
		grep "\"name\": *\"$1\"" | sed -n 's/.*"url": *"\([^"]*\/assets\/[0-9]*\)".*/\1/p' | head -1 | grep .
}
archive_url=$(asset_url "$archive") || die "release $tag has no archive for $target"
sums_url=$(asset_url SHA256SUMS) || die "release $tag has no SHA256SUMS"

say "downloading kiln $version for $target"
fetch "$archive_url" "$work/$archive" application/octet-stream
fetch "$sums_url" "$work/SHA256SUMS" application/octet-stream
expected=$(grep " $archive\$" "$work/SHA256SUMS" | cut -d' ' -f1)
[ -n "$expected" ] || die "SHA256SUMS does not list $archive"
actual=$(sha "$work/$archive")
[ "$expected" = "$actual" ] || die "checksum mismatch for $archive: expected $expected, got $actual"

mkdir -p "$install_dir" "$bin_dir"
tar -C "$work" -xzf "$work/$archive"
rm -rf "${install_dir:?}/${version:?}"
mv "$work/kiln-$version-$target" "$install_dir/$version"
ln -sfn "$install_dir/$version/bin/kiln" "$bin_dir/kiln"

say "installed $("$bin_dir/kiln" --version | head -1) to $install_dir/$version"
case ":$PATH:" in
*":$bin_dir:"*) ;;
*) say "add $bin_dir to PATH to run it as \`kiln\`" ;;
esac
if ! command -v cc >/dev/null 2>&1 && ! command -v clang >/dev/null 2>&1 && ! command -v gcc >/dev/null 2>&1 && ! command -v zig >/dev/null 2>&1; then
	say "native builds need a C compiler (clang, gcc, cc or zig); \`kiln run\` works without one"
fi
