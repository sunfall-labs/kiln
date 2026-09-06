#!/usr/bin/env bash
#
# Where a native binary spends its cycles, by symbol and by instruction.
#
# `sample` answers by symbol at a millisecond, which names a function but not what inside it is
# slow; once everything inlines into one emitted body that is the only question left. This
# records an Instruments CPU profile (a few hundred samples per second of run; a benchmark's
# two seconds are enough to rank symbols and to see whether a function has one hot spot or
# none), exports the profile table, and prints the hottest symbols and the hottest instruction
# offsets in the binary, which `objdump -d <binary>` then reads: an offset here is the address in
# the disassembly minus the image base (0x100000000 for a macOS executable).
#
# Usage: scripts/profile-native.sh <binary> [top-n]
#   KILN_PROFILE_TRACE=<path.trace> keeps the trace for Instruments.app.
#
# Only macOS: Instruments is the profiler this platform has, since Valgrind does not run on
# Apple Silicon. Build the binary first: `kiln --one-shot build bench/x.mjs --profile
# native-release --output /tmp/x.bin` (the profile flags keep symbols; `native-profile` adds -pg
# if a callgraph by function is wanted instead).
set -euo pipefail
binary="${1:?usage: profile-native.sh <binary> [top-n]}"
top="${2:-30}"
trace="${KILN_PROFILE_TRACE:-$(mktemp -d "${TMPDIR:-/tmp}/kiln-profile.XXXXXX")/profile.trace}"
here="$(cd "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
command -v xcrun >/dev/null || { echo "profile-native.sh: xcrun (Xcode command line tools) is required" >&2; exit 1; }
rm -rf "$trace"
xcrun xctrace record --template 'CPU Profiler' --output "$trace" --launch -- "$binary" >/dev/null 2>&1
export="$(mktemp "${TMPDIR:-/tmp}/kiln-profile.XXXXXX.xml")"
xcrun xctrace export --input "$trace" --xpath '/trace-toc/run[@number="1"]/data/table[@schema="cpu-profile"]' > "$export" 2>/dev/null
python3 "$here/xctrace-top.py" "$export" "$(basename -- "$binary")" "$top"
rm -f "$export"
[[ -n ${KILN_PROFILE_TRACE:-} ]] || rm -rf "$(dirname -- "$trace")"
