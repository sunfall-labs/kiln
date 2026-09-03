#!/usr/bin/env bash
#
# Where does a natively compiled server spend its time?
#
# Builds the module with `--profile native-profile` (release plus `gprof` instrumentation), runs it
# under the mixed-workload driver, and prints the flat profile. The program has to exit cleanly for
# `gmon.out` to be written, so a server module must close its own listener -- see the
# `KILN_PROFILE_SECONDS` note below.
#
# usage: scripts/profile-native.sh <module.mjs> [seconds] [top-n]
#
# Why gprof and not perf: `perf` needs a kernel-matched package, which a container on someone
# else's kernel does not get. `valgrind --tool=callgrind` is the other option here and needs no
# rebuild at all -- reach for it when you want exact instruction counts and can afford ~50x, and
# for this when you want wall-clock attribution at something near real speed.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

KILN_PROFILE_MODULE="${1:?usage: scripts/profile-native.sh <module.mjs> [seconds] [top-n]}"
KILN_PROFILE_SECONDS="${2:-12}"
KILN_PROFILE_TOP="${3:-25}"
KILN_PROFILE_PORT="${KILN_PROFILE_PORT:-18088}"
KILN_PROFILE_CLIENTS="${KILN_PROFILE_CLIENTS:-4}"
KILN_PROFILE_CONNS="${KILN_PROFILE_CONNS:-16}"
KILN_PROFILE_WORKLOAD="${KILN_PROFILE_WORKLOAD:-mixed}"

if ! command -v gprof >/dev/null 2>&1; then
	# shellcheck disable=SC2016
	printf 'gprof is not installed; `apt-get install binutils` provides it\n' >&2
	exit 1
fi

KILN_PROFILE_WORK="$(mktemp -d)"
kiln_profile_cleanup() {
	# A forked worker reparents to init and keeps the SO_REUSEPORT listener, which would serve --
	# and skew -- whatever runs next. Same reaping the benchmark harness learned to do.
	local held
	held=$(ss -lntpH "sport = :$KILN_PROFILE_PORT" 2>/dev/null |
		grep -oE 'pid=[0-9]+' | cut -d= -f2 | sort -u || true)
	[[ -n $held ]] && kill -9 "$held" 2>/dev/null || true
	rm -rf "$KILN_PROFILE_WORK"
}
trap kiln_profile_cleanup EXIT

printf 'building %s with native-profile\n' "$KILN_PROFILE_MODULE" >&2
KILN_PROFILE_BUILD=$(cargo run --quiet -p kiln-cli -- build --one-shot --node \
	--profile native-profile "$KILN_PROFILE_MODULE" 2>&1) || {
	printf '%s\n' "$KILN_PROFILE_BUILD" >&2
	exit 1
}
KILN_PROFILE_BINARY=$(printf '%s\n' "$KILN_PROFILE_BUILD" | awk '$1=="output"{print $2}')
if [[ ! -x $KILN_PROFILE_BINARY ]]; then
	printf 'no executable in the build output:\n%s\n' "$KILN_PROFILE_BUILD" >&2
	exit 1
fi

# gmon.out lands in the process's working directory.
(cd "$KILN_PROFILE_WORK" && "$KILN_PROFILE_BINARY" >/dev/null 2>&1) &
for _ in $(seq 1 50); do
	curl -s -o /dev/null "http://127.0.0.1:$KILN_PROFILE_PORT/" && break
	sleep 0.1
done

printf 'driving %s for %ss\n' "$KILN_PROFILE_WORKLOAD" "$KILN_PROFILE_SECONDS" >&2
bash "$(kiln_repo_root)/bench/server/drive-multi.sh" "$KILN_PROFILE_CLIENTS" "$KILN_PROFILE_CONNS" \
	$((KILN_PROFILE_SECONDS * 1000 - 3000)) "$KILN_PROFILE_PORT" "$KILN_PROFILE_WORKLOAD"
wait || true

if [[ ! -f $KILN_PROFILE_WORK/gmon.out ]]; then
	printf 'no gmon.out: the module has to close its listener so the program exits.\n' >&2
	# shellcheck disable=SC2016
	printf 'Add `setTimeout(() => server.close(), %s000)` to a profiling copy of it.\n' \
		"$KILN_PROFILE_SECONDS" >&2
	exit 1
fi

printf '\n=== flat profile, top %s ===\n' "$KILN_PROFILE_TOP"
gprof -b -p "$KILN_PROFILE_BINARY" "$KILN_PROFILE_WORK/gmon.out" 2>/dev/null |
	head -n $((KILN_PROFILE_TOP + 5))
