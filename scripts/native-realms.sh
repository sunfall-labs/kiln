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
# HOW. Each probe is built once by `kiln run --native`, which keeps the realm's virtual clock.
# The binary runs alone, then with `KILN_REALMS=N`, which launches N realms at once. By default
# realms share turn-scheduler lanes, and eligible turns migrate between lanes. Set
# `KILN_REALM_MIGRATE=0` to retain fixed scheduler lanes, or `KILN_REALM_SCHEDULER=0` to use
# the older dedicated-thread launcher.
# The launcher writes each realm's standard streams into one file (as `2>&1` does) and its exit
# status to `$KILN_REALMS_OUT/<index>.{out,status}`. Every realm's pair must match the lone run
# for N = 1, 2, and the core count.
#
# The oracle is the binary alone, not the interpreter: `native.sh` owns that comparison, and a
# probe that diverges there diverges here only if realms make it worse.
#
#   scripts/native-realms.sh [probe ...]      the whole corpus, or just these probes
#   scripts/native-realms.sh --repeat K [probe ...]
#                                             the stress mode: every realm count's run K times
#                                             (default counts: the cores and four times them)
#   KILN_NATIVE_GC_STRESS=1 ...               every run with the collector stressed on every
#                                             allocation (name a representative subset: minutes)
#   KILN_REALMS_COUNTS="1 2 8"                the realm counts to run (default: 1 2 and the cores)
#   KILN_NATIVE_REALMS_LOG_DIR=<directory>    retain per-probe streams and phase markers
#
# WHY REPEAT. A race between realms shows up in some runs and not others: realms issue 24's
# segfault -- every realm thread tearing down at once used one process-wide fallback heap -- took
# about one run in a thousand at 16 realms, and one pass over the corpus runs each count once. The
# stress mode runs each count K times and counts a process that died by a signal as diverged, with
# the run and the signal named, so `--repeat 1000` on one or two short probes is the check to run
# after touching anything realms share or anything a realm runs at its start or its teardown.
#
# Skipped, and counted as skipped: a probe calling `process.exit`, which ends the whole process
# and so every realm in it (a realm's own exit is `worker_threads`' business, issue 06); a probe
# that writes files at fixed paths, which every realm of one process shares; a probe the native
# backend refuses to build.
#
# Execute it; do not `bash` it (see AGENTS.md: macOS's bash 3.2 breaks the cleanup trap).
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
REPEAT=1
if [[ ${1:-} == --repeat ]]; then
	if [[ ! ${2:-} =~ ^[1-9][0-9]*$ ]]; then
		echo "native-realms.sh: --repeat needs a positive count of runs" >&2
		exit 2
	fi
	REPEAT="$2"
	shift 2
fi
if ((REPEAT > 1)); then
	COUNTS="${KILN_REALMS_COUNTS:-$CORES $((CORES * 4))}"
else
	COUNTS="${KILN_REALMS_COUNTS:-1 2 $CORES}"
fi

declare -a PROBES=("$@")
if ((${#PROBES[@]} == 0)); then
	shopt -s nullglob
	PROBES=(conformance/native/*.ts conformance/native/*.js conformance/native/*.cjs conformance/native/*.mjs)
fi
for probe in "${PROBES[@]}"; do
	if [[ ! -f $probe ]]; then
		echo "native-realms.sh: no such probe: $probe" >&2
		exit 2
	fi
done

KILN="${KILN_BIN:-}"
if [[ -z $KILN ]]; then
	if [[ -x "$(kiln_repo_root)/target/release/kiln" ]]; then
		KILN="$(kiln_repo_root)/target/release/kiln"
	else
		kiln_cargo build --locked -p kiln-cli >&2
		KILN="$(kiln_repo_root)/target/debug/kiln"
	fi
fi

# A run of N realms already keeps N cores busy; a few probes at once keeps the builds overlapped.
JOBS="${KILN_JOBS:-$(((CORES + 3) / 4))}"
((JOBS < 1)) && JOBS=1

if [[ -n ${KILN_NATIVE_REALMS_LOG_DIR:-} ]]; then
	mkdir -p "$KILN_NATIVE_REALMS_LOG_DIR"
	WORK_DIR="$(mktemp -d "$KILN_NATIVE_REALMS_LOG_DIR/run.XXXXXXXX")"
else
	WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/kiln-native-realms.XXXXXXXX")"
fi
cleanup() {
	if [[ ${BASHPID:-$$} -eq $$ ]]; then
		if [[ -n ${KILN_NATIVE_REALMS_LOG_DIR:-} ]]; then
			kiln_say "native realms evidence: $WORK_DIR"
		else
			rm -rf "$WORK_DIR"
		fi
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
	printf '%s\n' build > "$dir/phase"
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
	printf '%s\n' alone > "$dir/phase"
	env ${probe_env[@]+"${probe_env[@]}"} "$dir/program" > "$dir/alone.log" 2>&1 || alone_status=$?
	printf '%s\n' "$alone_status" > "$dir/alone.status"
	alone="$(<"$dir/alone.log")"

	local count index realm_status run process_status where
	for count in $COUNTS; do
		local out="$dir/realms-$count"
		for ((run = 1; run <= REPEAT; run++)); do
			rm -rf "$out"
			mkdir -p "$out"
			where="N=$count"
			((REPEAT > 1)) && where="N=$count run $run of $REPEAT"
			printf '%s\n' "$where" > "$dir/phase"
			# The process's own streams stay empty unless the host itself fails; they are kept.
			process_status=0
			env ${probe_env[@]+"${probe_env[@]}"} KILN_REALMS="$count" KILN_REALMS_OUT="$out" \
				"$dir/program" > "$out/process.log" 2>&1 || process_status=$?
			# A process a signal ended is a crash whatever its realms managed to write first.
			if ((process_status > 128)); then
				printf '  %s: the process died by signal %s (%s)\n' "$where" \
					"$(kill -l "$((process_status - 128))" 2>/dev/null || echo "$((process_status - 128))")" \
					"$(head -3 "$out/process.log" | tr '\n' ' ')" > "$dir/diverged"
				return 0
			fi
			for ((index = 0; index < count; index++)); do
				if [[ ! -f $out/$index.status ]]; then
					printf '  %s realm %s: no status (%s)\n' "$where" "$index" \
						"$(head -3 "$out/process.log" | tr '\n' ' ')" > "$dir/diverged"
					return 0
				fi
				realm_status="$(<"$out/$index.status")"
				if [[ $(<"$out/$index.out") != "$alone" ]] || ((realm_status != alone_status)); then
					{
						printf '  %s realm %s exited %s (alone: %s); its output against the lone run:\n' \
							"$where" "$index" "$realm_status" "$alone_status"
						diff <(printf '%s\n' "$alone") "$out/$index.out" | head -12 | sed 's/^/    /'
					} > "$dir/diverged"
					return 0
				fi
			done
		done
	done
	printf '%s\n' matched > "$dir/phase"
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

RUNS_NOTE=""
((REPEAT > 1)) && RUNS_NOTE="; $REPEAT runs each"
kiln_say "native realms (N = ${COUNTS// /, }$RUNS_NOTE${STRESS:+; KILN_GC_STRESS=$STRESS}): $matched matched, ${#SKIPPED[@]} skipped, $((${#DIVERGED[@]} / 2)) diverged"
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
