#!/usr/bin/env bash
#
# Run the third-party bundles in conformance/thirdparty against node, and fail on any divergence.
#
# WHY THIS EXISTS ALONGSIDE scripts/differential.sh. That one runs probes *we* wrote, so it can
# only find what someone thought to ask about. These are real libraries -- date-fns, immer, zod,
# semver, mustache, js-yaml -- written against the whole language rather than against what kiln
# happens to implement, and every one of them found something within seconds of first running.
# None of those was a refusal; each was a plausible answer that was wrong. Writing a string
# index, a Date setter, `Map.values()`, `array.length = 0`, and `===` on a date, are things a
# hand-written probe reaches only if you already suspect them.
#
# There is no "refused" outcome here. A library either produces node's bytes or it does not: the
# programs are far too large for a refusal to be a useful signal, and any of them stopping early
# is a failure whatever the reason.
#
# WHY IT RUNS EACH BUNDLE TWICE. Kiln has two engines, and for a long time this script exercised
# only the interpreter. That is how the native backend came to be missing ten `Number` statics and
# eight well-known symbols without anything noticing: `Number.MAX_VALUE` is the first property a
# bundled `zod` reads, so its absence refused the whole module, and no harness was asking. The
# native pass closes that. It is the same bundles and the same expected bytes -- the point is that
# a library is a far better interrogator of the builtin surface than any probe we would write.
#
# The two passes judge differently, on purpose. Interpreted, there is no acceptable outcome but
# node's bytes. Natively, a refusal is the honest state of a backend that is still being filled
# in, and it names what it lacks; it is reported and does not fail. A *divergence* fails in both,
# because a wrong answer is worse than an absent one -- a refusal stops the program where a wrong
# answer lets it continue on a bad value. Set KILN_THIRDPARTY_NATIVE=0 to skip the native pass
# when you only want the fast half.
#
# WHAT "NODE'S BYTES" INCLUDES. Both streams, and node's exit status. This compared output alone
# for a long time, and a bundle that printed exactly node's bytes and then exited 1 was a *match*
# to it -- which is how a CommonJS entry exiting 1 on success survived for weeks, invisible here
# and immediately fatal to anyone running the same program from a Makefile or CI. The status is
# compared against node's own rather than against zero, because a bundle node runs to a nonzero
# exit is expected to reach the same nonzero exit here. A refusal is still classified as a refusal
# before any of this: it exits nonzero by design, and that is not the disagreement being hunted.
#
# Bundles small enough to read are committed; the large ones are gitignored, and this skips them
# with a note rather than failing, because a checkout that has not run build.sh is not broken.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

BUNDLES="${KILN_THIRDPARTY_DIR:-conformance/thirdparty}"

if ! command -v node >/dev/null 2>&1; then
	kiln_say "SKIP: node is not on PATH, so there is nothing to compare against."
	exit 0
fi

KILN="${KILN_BIN:-}"
if [[ -z $KILN ]]; then
	if [[ -x "$(kiln_repo_root)/target/release/kiln" ]]; then
		KILN="$(kiln_repo_root)/target/release/kiln"
	else
		kiln_cargo build --locked -p kiln-cli >&2
		KILN="$(kiln_repo_root)/target/debug/kiln"
	fi
fi

BENCH=0
UPDATE=0
THRESHOLD="${KILN_THIRDPARTY_THRESHOLD:-25}"
MIN_DELTA_MS="${KILN_THIRDPARTY_MIN_DELTA_MS:-3.0}"
BASELINE="${KILN_THIRDPARTY_BASELINE:-bench/results/thirdparty-baseline.json}"
PROFILE="${KILN_THIRDPARTY_PROFILE:-native-release}"
PASSES="${PASSES:-5}"
WARMUP="${WARMUP:-1}"
TARGET_BUNDLES=""

while [[ $# -gt 0 ]]; do
	case "$1" in
		--bench | --timing | --baseline)
			BENCH=1
			shift
			;;
		--update)
			BENCH=1
			UPDATE=1
			shift
			;;
		--threshold)
			THRESHOLD="${2:?--threshold requires a percentage}"
			shift 2
			;;
		--min-delta-ms)
			MIN_DELTA_MS="${2:?--min-delta-ms requires milliseconds}"
			shift 2
			;;
		--baseline-file)
			BASELINE="${2:?--baseline-file requires a path}"
			shift 2
			;;
		--profile)
			PROFILE="${2:?--profile requires a profile name}"
			shift 2
			;;
		--passes)
			PASSES="${2:?--passes requires a count}"
			shift 2
			;;
		--warmup)
			WARMUP="${2:?--warmup requires a count}"
			shift 2
			;;
		--bundles)
			TARGET_BUNDLES="${2:?--bundles requires bundle names}"
			shift 2
			;;
		-h | --help)
			echo "Usage: scripts/thirdparty.sh [options]"
			echo ""
			echo "Options:"
			echo "  --bench, --timing, --baseline   Time thirdparty bundles and verify against baseline"
			echo "  --update                        Regenerate/update the committed baseline JSON"
			echo "  --threshold <pct>               Regression threshold percentage (default: 25)"
			echo "  --min-delta-ms <ms>             Minimum absolute delta in ms to consider regression (default: 3.0)"
			echo "  --baseline-file <path>          Path to baseline JSON (default: bench/results/thirdparty-baseline.json)"
			echo "  --profile <profile>             Native build profile (default: native-release)"
			echo "  --passes <count>                Timing iterations per bundle (default: 5)"
			echo "  --warmup <count>                Warmup iterations per bundle (default: 1)"
			echo "  --bundles <names>               Comma-separated list of bundle names to test"
			exit 0
			;;
		*)
			echo "Unknown argument: $1" >&2
			exit 1
			;;
	esac
done

# The four probes build.sh bundles as CommonJS for the *node* platform, because the libraries
# behind them reach `path`, `fs`, `os` and `util` and cannot be bundled for the neutral one at
# all. Running one of those without the Node Adapter is a harness bug rather than a finding: it
# reports "require is not defined", which says nothing about the library. Kept in step with the
# NODE_PLATFORM list in conformance/thirdparty/build.sh, which is where the decision is made.
NODE_PLATFORM=" typescript babel postcss react-ssr "

matched=0
missing=0
diverged=0
DIVERGENCES=()

JOBS="${KILN_JOBS:-$(nproc 2>/dev/null || echo 4)}"
if (( JOBS > 16 )); then
	JOBS=16
elif (( JOBS < 1 )); then
	JOBS=1
fi

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/kiln-thirdparty-results.XXXXXXXX")"
cleanup() {
	if [[ ${BASHPID:-$$} -eq $$ ]]; then
		rm -rf "$WORK_DIR"
	fi
}
trap cleanup EXIT

if [[ $BENCH -eq 1 ]]; then
	python3 - "$KILN" "$BUNDLES" "$BASELINE" "$WORK_DIR" "$UPDATE" "$THRESHOLD" "$MIN_DELTA_MS" "$PROFILE" "$PASSES" "$WARMUP" "$NODE_PLATFORM" "$TARGET_BUNDLES" <<'PY'
import json, os, statistics, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor

kiln, bundles_dir, baseline_path, work_dir, update_str, threshold_str, min_delta_str, profile, passes_str, warmup_str, node_platform_str, target_bundles_str = sys.argv[1:13]

update = update_str == "1"
threshold = float(threshold_str)
min_delta_ms = float(min_delta_str)
passes = max(1, int(passes_str))
warmup = max(0, int(warmup_str))
node_platform = set(node_platform_str.split())

DEFAULT_BASELINE_BUNDLES = [
    "postcss", "zod", "acorn", "chalk", "datefns", "dayjs", "immer",
    "marked", "ms", "mustache", "rxjs", "semver", "uuid", "vue-reactivity", "yaml"
]

if os.path.exists(baseline_path):
    with open(baseline_path) as handle:
        baseline = json.load(handle)
else:
    baseline = {"schema": 1, "profile": profile, "threshold_pct": threshold, "bundles": {}}

# Determine bundles to measure
if target_bundles_str:
    bundle_names = [b.strip() for b in target_bundles_str.split(",") if b.strip()]
elif baseline.get("bundles"):
    bundle_names = sorted(baseline["bundles"].keys())
else:
    bundle_names = DEFAULT_BASELINE_BUNDLES

print(f"thirdparty baseline (profile={profile}, threshold={threshold:.1f}%, min_delta={min_delta_ms:.1f}ms, passes={passes}, warmup={warmup}):")

measured = {}
regressions = []
gains = []

max_workers = min(len(bundle_names), int(os.environ.get("KILN_JOBS", os.cpu_count() or 4)))
if max_workers > 16:
    max_workers = 16
elif max_workers < 1:
    max_workers = 1

def build_one(name):
    bundle_path = os.path.join(bundles_dir, f"{name}.bundle.js")
    if not os.path.exists(bundle_path):
        return (name, None, "absent")
    bin_path = os.path.join(work_dir, f"{name}.bin")
    adapter = ["--node"] if name in node_platform else []
    build_cmd = [kiln, "--one-shot", *adapter, "build", bundle_path, "--profile", profile, "--output", bin_path]
    built = subprocess.run(build_cmd, capture_output=True, text=True)
    if built.returncode != 0 or not os.path.exists(bin_path):
        err_snippet = built.stderr.strip().splitlines()
        err_msg = err_snippet[-1] if err_snippet else f"exit {built.returncode}"
        return (name, None, err_msg)
    return (name, bin_path, None)

print(f"building {len(bundle_names)} bundles with {max_workers} worker(s)...", flush=True)
with ThreadPoolExecutor(max_workers=max_workers) as executor:
    build_results = {name: (bin_path, err) for name, bin_path, err in executor.map(build_one, bundle_names)}

time.sleep(0.5)

for index, name in enumerate(bundle_names, 1):
    bin_path, err_msg = build_results.get(name, (None, "unknown"))
    if err_msg == "absent":
        print(f"[{index}/{len(bundle_names)}] {name:16} (bundle absent on disk; skipped)", flush=True)
        continue
    if not bin_path:
        regressions.append(f"{name}: native compilation failed ({err_msg})")
        print(f"[{index}/{len(bundle_names)}] {name:16} COMPILE FAILED ({err_msg})", flush=True)
        continue

    # Warmup passes to avoid page-fault noise on fresh binaries
    for _ in range(max(2, warmup)):
        subprocess.run([bin_path], capture_output=True)

    # Execute passes
    times = []
    failed = False
    for _ in range(passes):
        t0 = time.perf_counter()
        proc = subprocess.run([bin_path], capture_output=True)
        t1 = time.perf_counter()
        if proc.returncode != 0:
            err_snippet = proc.stderr.decode("utf-8", errors="replace").strip().splitlines()
            err_msg = err_snippet[-1] if err_snippet else f"exit {proc.returncode}"
            regressions.append(f"{name}: native execution failed ({err_msg})")
            print(f"[{index}/{len(bundle_names)}] {name:16} RUN FAILED ({err_msg})", flush=True)
            failed = True
            break
        times.append((t1 - t0) * 1000.0)

    if failed or not times:
        continue

    median_ms = round(statistics.median(times), 2)
    min_ms = round(min(times), 2)
    measured[name] = {"median_ms": median_ms, "min_ms": min_ms}

    was = baseline.get("bundles", {}).get(name)
    if was and "median_ms" in was:
        was_ms = was["median_ms"]
        diff_ms = median_ms - was_ms
        pct = (diff_ms / was_ms) * 100.0 if was_ms > 0 else 0.0
        if diff_ms > min_delta_ms and pct > threshold:
            regressions.append(f"{name}: was {was_ms:.2f}ms, now {median_ms:.2f}ms (+{pct:.1f}% > {threshold:.1f}% threshold)")
            mark = f"  REGRESSED (+{pct:.1f}%)"
        elif diff_ms < -min_delta_ms and pct < -threshold:
            gains.append(f"{name}: was {was_ms:.2f}ms, now {median_ms:.2f}ms ({pct:.1f}%)")
            mark = f"  {diff_ms:+.2f}ms (gain)"
        else:
            mark = f"  ({pct:+.1f}%)"
        print(f"[{index}/{len(bundle_names)}] {name:16} {median_ms:.2f}ms (baseline {was_ms:.2f}ms){mark}", flush=True)
    else:
        print(f"[{index}/{len(bundle_names)}] {name:16} {median_ms:.2f}ms (new)", flush=True)

if update:
    baseline["schema"] = 1
    baseline["profile"] = profile
    baseline["threshold_pct"] = threshold
    baseline.setdefault("bundles", {})
    baseline["bundles"].update(measured)
    os.makedirs(os.path.dirname(os.path.abspath(baseline_path)), exist_ok=True)
    with open(baseline_path, "w") as handle:
        json.dump(baseline, handle, indent=2, sort_keys=True)
        handle.write("\n")
    print(f"\nwrote {baseline_path}: {len(baseline['bundles'])} bundle(s) recorded")
    sys.exit(0)

for line in gains:
    print(f"gain     {line}")
for line in regressions:
    print(f"REGRESS  {line}", file=sys.stderr)

if regressions:
    regressed_names = [r.split(":")[0].strip() for r in regressions]
    sys.exit(f"\n{len(regressions)} bundle(s) regressed: {', '.join(regressed_names)}; accept them with --update if intended")

total_measured = len(measured)
print(f"\nno regressions across {total_measured} bundle(s)" + (f", {len(gains)} gained" if gains else ""))
PY
	exit 0
fi

run_interpreted() {
	trap - EXIT
	local probe="$1"
	local name
	name="$(basename "$probe" .mjs)"
	local bundle="$BUNDLES/$name.bundle.js"
	if [[ ! -f $bundle ]]; then
		touch "$WORK_DIR/$name.interp_missing"
		return 0
	fi
	local adapter=""
	if [[ $NODE_PLATFORM == *" $name "* ]]; then
		adapter="--node"
	fi
	# The two streams are compared separately, not merged with `2>&1`. Merging made the test
	# depend on interleaving, and the interleaving legitimately differs: kiln captures a program's
	# stdout and writes it when the run ends, so its stderr lands after all of its stdout where
	# node's is written as it happens. A bundled `vue-reactivity` emits one `console.warn` and
	# diverged on ordering alone. Per-stream also checks that each line went to the right place,
	# which merging could not see at all.
	# Each engine's status comes from its stdout run; the second run exists only to separate the
	# streams and is the same program over the same inputs.
	local expected expected_err actual actual_err
	local expected_status=0 actual_status=0
	expected="$(TZ=UTC node "$bundle" 2>/dev/null)" || expected_status=$?
	expected_err="$(TZ=UTC node "$bundle" 2>&1 1>/dev/null || true)"
	actual="$("$KILN" --one-shot ${adapter:+"$adapter"} run "$bundle" 2>/dev/null)" || actual_status=$?
	actual_err="$("$KILN" --one-shot ${adapter:+"$adapter"} run "$bundle" 2>&1 1>/dev/null || true)"
	if [[ $actual == "$expected" && $actual_err == "$expected_err" ]] &&
		((actual_status == expected_status)); then
		touch "$WORK_DIR/$name.interp_matched"
	else
		{
			printf '%s\n' "    node: $(printf '%s' "$expected" | head -3 | tr '\n' '|') [exit $expected_status]"
			printf '%s\n' "    kiln: $(printf '%s' "$actual" | head -3 | tr '\n' '|') [exit $actual_status]"
			if [[ $actual == "$expected" && $actual_err == "$expected_err" ]]; then
				printf '%s\n' "    the bytes agree; only the exit status differs"
			fi
		} > "$WORK_DIR/$name.interp_diverged"
	fi
}

running=0
for probe in "$BUNDLES"/probes/*.mjs; do
	run_interpreted "$probe" &
	running=$((running + 1))
	if (( running >= JOBS )); then
		wait -n
		running=$((running - 1))
	fi
done
wait

for probe in "$BUNDLES"/probes/*.mjs; do
	name="$(basename "$probe" .mjs)"
	if [[ -f "$WORK_DIR/$name.interp_missing" ]]; then
		missing=$((missing + 1))
	elif [[ -f "$WORK_DIR/$name.interp_matched" ]]; then
		matched=$((matched + 1))
	elif [[ -f "$WORK_DIR/$name.interp_diverged" ]]; then
		diverged=$((diverged + 1))
		DIVERGENCES+=("  $name")
		while IFS= read -r line; do
			DIVERGENCES+=("$line")
		done < "$WORK_DIR/$name.interp_diverged"
	fi
done

kiln_say "thirdparty: $matched matched, $diverged diverged, $missing not built"

if ((missing > 0)); then
	kiln_say "  ($missing bundle(s) absent; run conformance/thirdparty/build.sh to produce them)"
fi

# Reported here, but not exited on yet: the native pass below must run even when the interpreted
# one has found something. Gating it behind a green interpreted run would rebuild the very hole
# this script was widened to close -- a single known divergence would silently stop anyone from
# ever seeing the native half. The exit status is decided once, at the end, on both passes.
if ((${#DIVERGENCES[@]} > 0)); then
	kiln_say ""
	kiln_say "DIVERGED - a real library got a different answer here than under node:"
	printf '%s\n' "${DIVERGENCES[@]}" | while IFS= read -r line; do kiln_say "$line"; done
fi

if [[ ${KILN_THIRDPARTY_NATIVE:-1} == 0 ]]; then
	((diverged > 0)) && exit 1
	exit 0
fi

native_matched=0
native_refused=0
native_diverged=0
REFUSALS=()
NATIVE_DIVERGENCES=()

run_native() {
	trap - EXIT
	local probe="$1"
	local name
	name="$(basename "$probe" .mjs)"
	local bundle="$BUNDLES/$name.bundle.js"
	if [[ ! -f $bundle ]]; then
		return 0
	fi
	local adapter=""
	if [[ $NODE_PLATFORM == *" $name "* ]]; then
		adapter="--node"
	fi
	local expected expected_err actual actual_err
	local expected_status=0 actual_status=0
	expected="$(TZ=UTC node "$bundle" 2>/dev/null)" || expected_status=$?
	expected_err="$(TZ=UTC node "$bundle" 2>&1 1>/dev/null || true)"
	actual="$("$KILN" --one-shot ${adapter:+"$adapter"} run --native "$bundle" 2>/dev/null)" || actual_status=$?
	actual_err="$("$KILN" --one-shot ${adapter:+"$adapter"} run --native "$bundle" 2>&1 1>/dev/null || true)"
	if [[ $actual == "$expected" && $actual_err == "$expected_err" ]] &&
		((actual_status == expected_status)); then
		touch "$WORK_DIR/$name.native_matched"
	# No colon required: a refusal that names nothing still prints `native backend refusal` and
	# stops there, and matching on the colon classified those as divergences -- the one outcome
	# that must never be wrong, because a divergence is what fails this script.
	#
	# An uncaught `EvalError` about code generation belongs here too. The realm has no compiler
	# and says so (ADR 0045); a library with a fallback takes it, and one whose whole job is
	# compiling schemas correctly has nowhere to go. That is a capability this realm lacks and
	# names, not a wrong answer.
	elif [[ $actual_err == *"native backend refusal"* || $actual_err == *"unsupported native emission"* ||
		$actual_err == *"EvalError: Code generation from strings disallowed"* ]]; then
		local refusal_summary reason
		# `|| true` because a refusal that names nothing makes grep exit 1, and under `set -e`
		# with pipefail that killed the script mid-report -- silently, after the interpreted
		# summary had printed, which read exactly like the native pass not existing.
		reason="$(printf '%s' "$actual_err" | grep -oE '(operation=[^ ]+( detail=[^=]*)?|unsupported native emission: .*)' | head -1 || true)"
		if [[ -z $reason && $actual_err == *"Code generation from strings disallowed"* ]]; then
			reason="requires runtime code generation, which this realm has none of (ADR 0045)"
		fi
		# An anonymous refusal is worth saying out loud rather than printing a blank line: the
		# entire value of refusing instead of guessing is that the refusal names what it lacked.
		[[ -n $reason ]] || reason="(refused without naming anything -- a diagnostic bug)"
		refusal_summary="$(printf '%-14s %s' "$name" "$reason")"
		printf '%s\n' "$refusal_summary" > "$WORK_DIR/$name.native_refused"
	else
		{
			printf '%s\n' "    node:   $(printf '%s' "$expected" | head -3 | tr '\n' '|') [exit $expected_status]"
			printf '%s\n' "    native: $(printf '%s' "$actual" | head -3 | tr '\n' '|') [exit $actual_status]"
			if [[ $actual == "$expected" && $actual_err == "$expected_err" ]]; then
				printf '%s\n' "    the bytes agree; only the exit status differs"
			fi
		} > "$WORK_DIR/$name.native_diverged"
	fi
}

running=0
for probe in "$BUNDLES"/probes/*.mjs; do
	run_native "$probe" &
	running=$((running + 1))
	if (( running >= JOBS )); then
		wait -n
		running=$((running - 1))
	fi
done
wait

for probe in "$BUNDLES"/probes/*.mjs; do
	name="$(basename "$probe" .mjs)"
	if [[ -f "$WORK_DIR/$name.native_matched" ]]; then
		native_matched=$((native_matched + 1))
	elif [[ -f "$WORK_DIR/$name.native_refused" ]]; then
		native_refused=$((native_refused + 1))
		REFUSALS+=("  $(<"$WORK_DIR/$name.native_refused")")
	elif [[ -f "$WORK_DIR/$name.native_diverged" ]]; then
		native_diverged=$((native_diverged + 1))
		NATIVE_DIVERGENCES+=("  $name")
		while IFS= read -r line; do
			NATIVE_DIVERGENCES+=("$line")
		done < "$WORK_DIR/$name.native_diverged"
	fi
done

kiln_say "thirdparty (native): $native_matched matched, $native_refused refused, $native_diverged diverged"

if ((native_refused > 0)); then
	kiln_say ""
	kiln_say "Refused by the native backend (the backlog, not a failure):"
	printf '%s\n' "${REFUSALS[@]}" | while IFS= read -r line; do kiln_say "$line"; done
fi

if ((native_diverged > 0)); then
	kiln_say ""
	kiln_say "DIVERGED natively - the compiled program answered, and answered wrongly:"
	printf '%s\n' "${NATIVE_DIVERGENCES[@]}" | while IFS= read -r line; do kiln_say "$line"; done
fi

if ((diverged > 0 || native_diverged > 0)); then
	exit 1
fi
