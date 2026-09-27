#!/usr/bin/env bash
#
# Run each conformance/native probe as several realms at once in one native process, and fail
# unless every realm prints exactly what the program prints alone (realms issue 05).
#
# WHAT IT ASKS. `scripts/native.sh` asks whether the native backend agrees with the interpreter.
# This asks the question that only exists once realms share a process: whether realms running side
# by side on their own threads -- sharing keys, shapes, inline caches and method paths (ADR 0071)
# -- can see each other. A realm that learns a cache entry another realm then misreads, a slot
# rewritten under another thread's reader, a key interned into the wrong realm: each shows up as a
# realm printing something the program alone does not.
#
# HOW. Each probe is built once, by `kiln run --native` (the realm keeps its virtual clock). The
# binary runs alone, and then with `KILN_REALMS=N`, which makes
# its launcher run N realms of the program at once, each pinned to its own thread of the runtime's
# `WorkerPool` (`kiln_realm_run_pinned`), and write each realm's standard streams (both into one
# file, as `2>&1` does) and exit status to `$KILN_REALMS_OUT/<index>.{out,status}`. Every realm's
# pair must be the lone run's, for N = 1 (the pinned host itself), 2, and the core count.
#
# The oracle is the binary alone, not the interpreter: `native.sh` owns that comparison, and a
# probe that diverges there diverges here only if realms make it worse.
#
#   scripts/native-realms.sh [probe ...]      the whole corpus, or just these probes
#   KILN_NATIVE_GC_STRESS=1 ...               every run with the collector stressed on every
#                                             allocation (name a representative subset: minutes)
#   KILN_REALMS_COUNTS="1 2 8"                the realm counts to run (default: 1 2 and the cores)
#
# Skipped, and counted as skipped: a probe calling `process.exit`, which ends the whole process
# and so every realm in it (a realm's own exit is `worker_threads`' business, issue 06); a probe
# that writes files at fixed paths, which every realm of one process shares; a probe the native
# backend refuses to build.
#
# Execute it; do not `bash` it (see CLAUDE.md: macOS's bash 3.2 breaks the cleanup trap).
if [[ ${BASH_VERSINFO[0]} -lt 4 ]]; then
	for candidate in /opt/homebrew/bin/bash /usr/local/bin/bash "$(command -v bash)"; do
		# shellcheck disable=SC2016
		if [[ -x $candidate ]] && [[ $("$candidate" -c 'echo ${BASH_VERSINFO[0]}') -ge 4 ]]; then
			exec "$candidate" "${BASH_SOURCE[0]}" "$@"
		fi
	done
	echo "native-realms.sh: needs bash 4 or newer; install one (brew install bash)" >&2
	exit 1
fi

set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

STRESS="${KILN_NATIVE_GC_STRESS:-}"
CORES="$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)"
COUNTS="${KILN_REALMS_COUNTS:-1 2 $CORES}"

KILN="${KILN_BIN:-}"
if [[ -z $KILN ]]; then
	if [[ -x "$(kiln_repo_root)/target/release/kiln" ]]; then
		KILN="$(kiln_repo_root)/target/release/kiln"
	else
		kiln_cargo build --locked -p kiln-cli >&2
		KILN="$(kiln_repo_root)/target/debug/kiln"
	fi
fi

declare -a PROBES=("$@")
if ((${#PROBES[@]} == 0)); then
	shopt -s nullglob
	PROBES=(conformance/native/*.ts conformance/native/*.js conformance/native/*.cjs conformance/native/*.mjs)
fi

# A run of N realms already keeps N cores busy; a few probes at once keeps the builds overlapped.
JOBS="${KILN_JOBS:-$(((CORES + 3) / 4))}"
((JOBS < 1)) && JOBS=1

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/kiln-native-realms.XXXXXXXX")"
cleanup() {
	if [[ ${BASHPID:-$$} -eq $$ ]]; then
		rm -rf "$WORK_DIR"
	fi
}
trap cleanup EXIT

run_probe() {
	trap - EXIT
	local probe="$1"
	local name adapter="" profile="" native_directive directive
	name="$(basename -- "$probe")"
	[[ $name == *.cjs || $name == *.mjs ]] && adapter="--node"
	name="${name%.*}"
	local dir="$WORK_DIR/$name"
	mkdir -p "$dir"

	if grep -q 'process\.exit' "$probe"; then
		echo "calls process.exit, which ends every realm in the process" > "$dir/skipped"
		return 0
	fi
	if grep -qE 'writeFile|appendFile|mkdirSync|rmSync|rmdirSync|unlink' "$probe"; then
		echo "writes a file at a fixed path, which realms in one process share" > "$dir/skipped"
		return 0
	fi
	# The same directives `native.sh` honours: an environment for every run, and a profile.
	local -a probe_env=()
	directive="$(sed -n '1s|^// native-env: ||p' "$probe")"
	[[ -n $directive ]] && read -r -a probe_env <<<"$directive"
	native_directive="$(head -5 -- "$probe" | grep -m1 -oE 'kiln-native:.*' || true)"
	if [[ $native_directive =~ profile=([a-z-]+) ]]; then
		profile="${BASH_REMATCH[1]}"
	fi
	[[ -n $STRESS ]] && probe_env+=("KILN_GC_STRESS=$STRESS")

	# The binary `kiln run --native` builds, not `kiln build`'s: that one keeps the realm's virtual
	# clock (ADR 0044), which is what makes a probe's output the same on every run -- a shipped
	# binary keeps wall time. `KILN_NATIVE_KEEP_WORK` leaves the build directory to copy it from.
	local kept built program=""
	KILN_NATIVE_KEEP_WORK=1 env ${probe_env[@]+"${probe_env[@]}"} "$KILN" --one-shot \
		${adapter:+"$adapter"} run --native ${profile:+--profile "$profile"} "$probe" > "$dir/build.log" 2>&1 || true
	while read -r kept; do
		for built in "$kept/run-graph" "$kept/program"; do
			[[ -z $program && -x $built ]] && cp "$built" "$dir/program" && program=1
		done
		rm -rf "$kept"
	done < <(sed -n 's/^kiln: kept native build directory \(.*\) (KILN_NATIVE_KEEP_WORK is set)$/\1/p' "$dir/build.log")
	if [[ -z $program ]]; then
		echo "the native build did not complete: $(grep -m1 '^kiln: native' "$dir/build.log" || true)" > "$dir/skipped"
		return 0
	fi

	local alone alone_status=0
	alone="$(env ${probe_env[@]+"${probe_env[@]}"} "$dir/program" 2>&1)" || alone_status=$?

	local count index realm_status
	for count in $COUNTS; do
		local out="$dir/realms-$count"
		mkdir -p "$out"
		# The process's own streams stay empty unless the host itself fails; they are kept.
		env ${probe_env[@]+"${probe_env[@]}"} KILN_REALMS="$count" KILN_REALMS_OUT="$out" \
			"$dir/program" > "$out/process.log" 2>&1 || true
		for ((index = 0; index < count; index++)); do
			if [[ ! -f $out/$index.status ]]; then
				printf '  N=%s realm %s: no status (%s)\n' "$count" "$index" \
					"$(head -3 "$out/process.log" | tr '\n' ' ')" > "$dir/diverged"
				return 0
			fi
			realm_status="$(<"$out/$index.status")"
			if [[ $(<"$out/$index.out") != "$alone" ]] || ((realm_status != alone_status)); then
				{
					printf '  N=%s realm %s exited %s (alone: %s); its output against the lone run:\n' \
						"$count" "$index" "$realm_status" "$alone_status"
					diff <(printf '%s\n' "$alone") "$out/$index.out" | head -12 | sed 's/^/    /'
				} > "$dir/diverged"
				return 0
			fi
		done
	done
	touch "$dir/matched"
}

running=0
for probe in "${PROBES[@]}"; do
	run_probe "$probe" &
	running=$((running + 1))
	if ((running >= JOBS)); then
		wait -n || true
		running=$((running - 1))
	fi
done
wait || true

matched=0
declare -a SKIPPED=() DIVERGED=()
for probe in "${PROBES[@]}"; do
	name="$(basename -- "$probe")"
	name="${name%.*}"
	dir="$WORK_DIR/$name"
	if [[ -f $dir/matched ]]; then
		matched=$((matched + 1))
	elif [[ -f $dir/skipped ]]; then
		SKIPPED+=("$(printf '  %-32s %s' "$name" "$(<"$dir/skipped")")")
	elif [[ -f $dir/diverged ]]; then
		DIVERGED+=("$name" "$(<"$dir/diverged")")
	else
		DIVERGED+=("$name" "  the probe's run left no verdict")
	fi
done

kiln_say "native realms (N = ${COUNTS// /, }${STRESS:+; KILN_GC_STRESS=$STRESS}): $matched matched, ${#SKIPPED[@]} skipped, $((${#DIVERGED[@]} / 2)) diverged"
if ((${#SKIPPED[@]} > 0)); then
	kiln_say ""
	kiln_say "Skipped:"
	printf '%s\n' "${SKIPPED[@]}" | while IFS= read -r line; do kiln_say "$line"; done
fi
if ((${#DIVERGED[@]} > 0)); then
	kiln_say ""
	kiln_say "DIVERGED - a realm running beside others printed what the program alone does not:"
	printf '%s\n' "${DIVERGED[@]}" | while IFS= read -r line; do kiln_say "$line"; done
	exit 1
fi
