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
# `wait -n` and `BASHPID` need bash 4.3 or newer. macOS ships 3.2 as /bin/bash, and under it this
# script's cleanup trap deletes the shared results directory instead of its own while the script
# still exits 0. Re-exec once under a bash that has them, the way scripts/gate.sh does.
if [[ ${BASH_VERSINFO[0]} -lt 4 ]]; then
	for candidate in /opt/homebrew/bin/bash /usr/local/bin/bash "$(command -v bash)"; do
		# shellcheck disable=SC2016
		if [[ -x $candidate ]] && [[ $("$candidate" -c 'echo ${BASH_VERSINFO[0]}') -ge 4 ]]; then
			exec "$candidate" "${BASH_SOURCE[0]}" "$@"
		fi
	done
	echo "thirdparty.sh: needs bash 4 or newer; install one (brew install bash)" >&2
	exit 1
fi

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
# These programs finish in 2-20ms, where a five-sample median can be moved across the gate's
# 3ms floor by one scheduler interruption. Twenty-one samples take only a few seconds for the
# whole corpus and made consecutive full runs agree; three warmups keep fresh-page noise out.
PASSES="${PASSES:-21}"
WARMUP="${WARMUP:-3}"
TARGET_BUNDLES=""
INCLUDE_POSTCSS=0
WAIT_FOR_RELEASE=""
WAIT_OWNER_PID=""

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
		--include-postcss)
			INCLUDE_POSTCSS=1
			shift
			;;
		--wait-for-release)
			WAIT_FOR_RELEASE="${2:?--wait-for-release requires a marker path}"
			shift 2
			;;
		--wait-owner-pid)
			WAIT_OWNER_PID="${2:?--wait-owner-pid requires a process id}"
			shift 2
			;;
		-h | --help)
			echo "Usage: scripts/thirdparty.sh [options]"
			echo ""
			echo "Options:"
			echo "  --bench, --timing, --baseline   Time thirdparty bundles and verify against baseline"
			echo "  --update                        Update this platform's baseline, preserving other platforms"
			echo "  --threshold <pct>               Regression threshold percentage (default: 25)"
			echo "  --min-delta-ms <ms>             Minimum absolute delta in ms to consider regression (default: 3.0)"
			echo "  --baseline-file <path>          Path to baseline JSON (default: bench/results/thirdparty-baseline.json)"
			echo "  --profile <profile>             Native build profile (default: native-release)"
			echo "  --passes <count>                Timing iterations per bundle (default: 21)"
			echo "  --warmup <count>                Warmup iterations per bundle (default: 3)"
			echo "  --bundles <names>               Comma-separated list of bundle names to test"
			echo "  --include-postcss               Include postcss in the otherwise routine benchmark selection"
			echo "  --wait-for-release <path>       Wait for gate marker after compilation, before timing"
			echo "  --wait-owner-pid <pid>          Fail promptly if the gate owning the marker exits"
			exit 0
			;;
		*)
			echo "Unknown argument: $1" >&2
			exit 1
			;;
	esac
done

if [[ -n $WAIT_OWNER_PID && -z $WAIT_FOR_RELEASE ]]; then
	echo "thirdparty.sh: --wait-owner-pid requires --wait-for-release" >&2
	exit 2
fi

if [[ -n $WAIT_FOR_RELEASE && $BENCH -eq 0 ]]; then
	echo "thirdparty.sh: --wait-for-release requires --bench" >&2
	exit 2
fi

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

WORK_TMPDIR="${KILN_THIRDPARTY_WORK_TMPDIR:-${TMPDIR:-/tmp}}"
mkdir -p "$WORK_TMPDIR"
WORK_DIR="$(mktemp -d "$WORK_TMPDIR/kiln-thirdparty-results.XXXXXXXX")"
cleanup() {
	if [[ ${BASHPID:-$$} -eq $$ ]]; then
		rm -rf "$WORK_DIR"
	fi
}
trap cleanup EXIT

if [[ $BENCH -eq 1 ]]; then
python3 - "$KILN" "$BUNDLES" "$BASELINE" "$WORK_DIR" "$UPDATE" "$THRESHOLD" "$MIN_DELTA_MS" "$PROFILE" "$PASSES" "$WARMUP" "$NODE_PLATFORM" "$TARGET_BUNDLES" "$INCLUDE_POSTCSS" "$WAIT_FOR_RELEASE" "$WAIT_OWNER_PID" <<'PY'
import datetime, errno, hashlib, json, os, platform, re, shlex, statistics, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor, as_completed

kiln, bundles_dir, baseline_path, work_dir, update_str, threshold_str, min_delta_str, profile, passes_str, warmup_str, node_platform_str, target_bundles_str, include_postcss_str, wait_for_release, wait_owner_pid_str = sys.argv[1:16]

update = update_str == "1"
threshold = float(threshold_str)
min_delta_ms = float(min_delta_str)
passes = max(1, int(passes_str))
warmup = max(0, int(warmup_str))
node_platform = set(node_platform_str.split())
production_profiles = {"native-release", "native-release-size", "native-size", "native-profile"}
profile_env = os.environ.copy()
profile_env["TZ"] = "UTC"
if profile in production_profiles:
    # Keep the Node oracle, compiler's .env selection, and the generated runtime's
    # KILN_NODE_ENV_PRODUCTION value aligned, as in conformance/npm-compat/runner.py.
    profile_env["NODE_ENV"] = "production"

DEFAULT_BASELINE_BUNDLES = [
    "postcss", "zod", "acorn", "chalk", "datefns", "dayjs", "immer",
    "marked", "ms", "mustache", "rxjs", "semver", "uuid", "vue-reactivity", "yaml"
]

if os.path.exists(baseline_path):
    with open(baseline_path) as handle:
        baseline = json.load(handle)
else:
    baseline = {"schema": 2, "threshold_pct": threshold, "platforms": {}}

def first_line(argv):
    try:
        result = subprocess.run(argv, capture_output=True, text=True, timeout=3)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return result.stdout.splitlines()[0].strip() if result.returncode == 0 and result.stdout.splitlines() else None

def cpu_identity():
    if platform.system() == "Darwin":
        return first_line(["sysctl", "-n", "machdep.cpu.brand_string"]) or platform.processor() or "unknown"
    if platform.system() == "Linux":
        try:
            with open("/proc/cpuinfo", encoding="utf-8") as cpuinfo:
                for line in cpuinfo:
                    key, sep, value = line.partition(":")
                    if sep and key.strip().lower() in {"model name", "hardware"}:
                        return " ".join(value.split())
        except OSError:
            pass
    return platform.processor() or "unknown"

def key_part(value):
    return re.sub(r"[^a-z0-9]+", "-", value.lower()).strip("-") or "unknown"

def current_platform():
    system = platform.system()
    machine = platform.machine()
    cpu_model = cpu_identity()
    details = {
        "system": system,
        "release": platform.release(),
        "machine": machine,
        "cpu_model": cpu_model,
    }
    key_parts = [system, machine, cpu_model]
    if system == "Darwin":
        details["machine_model"] = first_line(["sysctl", "-n", "hw.model"])
        if details["machine_model"]:
            key_parts.append(details["machine_model"])
    key_parts.append(profile)
    key = "-".join(map(key_part, key_parts))
    return key, details

def file_sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()

def git_output(*args):
    try:
        result = subprocess.run(["git", *args], capture_output=True, text=True, timeout=5)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return result.stdout.strip() if result.returncode == 0 else None

def tool_version(command):
    return first_line([*shlex.split(command), "--version"]) if command else None

platform_key, platform_details = current_platform()
platforms = baseline.get("platforms", {}) if baseline.get("schema") == 2 else {}
platform_record = platforms.get(platform_key)

if not update:
    if baseline.get("schema") != 2:
        sys.exit("thirdparty baseline has no platform provenance (legacy schema); refusing an incomparable timing check")
    if platform_record is None:
        available = ", ".join(sorted(platforms)) or "none"
        sys.exit(
            f"no thirdparty baseline for platform/profile {platform_key}; available: {available}. "
            "Cross-platform timings are incomparable; collect this platform with --update after correctness checks."
        )
    if platform_record.get("profile") != profile:
        sys.exit(
            f"thirdparty baseline profile mismatch for {platform_key}: "
            f"baseline={platform_record.get('profile')!r}, requested={profile!r}"
        )

# Determine bundles to measure
if target_bundles_str:
    bundle_names = [b.strip() for b in target_bundles_str.split(",") if b.strip()]
elif platform_record and platform_record.get("bundles"):
    bundle_names = sorted(platform_record["bundles"].keys())
elif baseline.get("schema") == 1 and update and baseline.get("bundles"):
    bundle_names = sorted(baseline["bundles"].keys())
else:
    bundle_names = DEFAULT_BASELINE_BUNDLES

include_postcss = include_postcss_str == "1" or (update and not target_bundles_str)
deferred_bundles = []
if not target_bundles_str and not include_postcss and "postcss" in bundle_names:
    bundle_names.remove("postcss")
    deferred_bundles.append("postcss")

if target_bundles_str:
    suite = "targeted"
elif include_postcss:
    suite = "full"
else:
    suite = "routine"
present_bundles = [
    name for name in bundle_names
    if os.path.exists(os.path.join(bundles_dir, f"{name}.bundle.js"))
]
absent_bundles = [name for name in bundle_names if name not in present_bundles]
if not present_bundles:
    sys.exit(
        f"thirdparty benchmark selected {len(bundle_names)} bundle(s), but none are present; "
        "refusing an empty timing comparison or baseline update"
    )
if not update:
    unverifiable = []
    changed_inputs = []
    for name in present_bundles:
        expected = platform_record.get("bundles", {}).get(name, {}).get("workload_sha256")
        actual = file_sha256(os.path.join(bundles_dir, f"{name}.bundle.js"))
        if expected is None:
            unverifiable.append(name)
        elif expected != actual:
            changed_inputs.append(name)
    if unverifiable or changed_inputs:
        details = []
        if unverifiable:
            details.append("missing workload hashes: " + ", ".join(unverifiable))
        if changed_inputs:
            details.append("workload hash changed: " + ", ".join(changed_inputs))
        sys.exit(
            f"thirdparty baseline is not comparable on {platform_key}: {'; '.join(details)}. "
            "Refresh this platform deliberately with --update after reviewing workload changes."
        )
deferred_summary = ", ".join(deferred_bundles) if deferred_bundles else "none"
print(
    f"coverage: suite={suite}, selected={len(bundle_names)} ({len(present_bundles)} present, "
    f"{len(absent_bundles)} absent), deferred={len(deferred_bundles)} ({deferred_summary})"
)

print(f"thirdparty baseline (profile={profile}, threshold={threshold:.1f}%, min_delta={min_delta_ms:.1f}ms, passes={passes}, warmup={warmup}):")

measured = {}
regressions = []
gains = []
measurement_failures = []

build_jobs = os.environ.get("KILN_THIRDPARTY_BUILD_JOBS", os.environ.get("KILN_JOBS", os.cpu_count() or 4))
max_workers = min(len(bundle_names), int(build_jobs))
if max_workers > 16:
    max_workers = 16
elif max_workers < 1:
    max_workers = 1

def build_one(name):
    started = time.perf_counter()
    bundle_path = os.path.join(bundles_dir, f"{name}.bundle.js")
    if not os.path.exists(bundle_path):
        return (name, None, "absent", 0.0)
    bin_path = os.path.join(work_dir, f"{name}.bin")
    adapter = ["--node"] if name in node_platform else []
    build_cmd = [kiln, "--one-shot", *adapter, "build", bundle_path, "--profile", profile, "--output", bin_path]
    built = subprocess.run(build_cmd, capture_output=True, text=True, env=profile_env)
    elapsed_ms = (time.perf_counter() - started) * 1000.0
    if built.returncode != 0 or not os.path.exists(bin_path):
        err_snippet = built.stderr.strip().splitlines()
        err_msg = err_snippet[-1] if err_snippet else f"exit {built.returncode}"
        return (name, None, err_msg, elapsed_ms)
    return (name, bin_path, None, elapsed_ms)

print(f"building {len(bundle_names)} bundles with {max_workers} worker(s)...", flush=True)
build_started = time.perf_counter()
build_results = {}
with ThreadPoolExecutor(max_workers=max_workers) as executor:
    futures = [executor.submit(build_one, name) for name in bundle_names]
    for future in as_completed(futures):
        name, bin_path, err, elapsed_ms = future.result()
        build_results[name] = (bin_path, err, elapsed_ms)
        if err != "absent":
            print(f"  build {name:16} {elapsed_ms / 1000.0:.2f}s", flush=True)
build_elapsed = time.perf_counter() - build_started
print(f"bundle build pool completed in {build_elapsed:.2f}s", flush=True)

if wait_for_release:
    print(f"waiting for gate release marker: {wait_for_release}", flush=True)
    barrier_started = time.perf_counter()
    # Standalone callers retain the short default. The gate supplies its own longer cap and
    # owner PID, so a slow but live correctness phase is not confused with an abandoned gate.
    try:
        release_timeout = int(os.environ.get("KILN_THIRDPARTY_RELEASE_TIMEOUT_SECONDS", "900"))
    except ValueError:
        sys.exit("KILN_THIRDPARTY_RELEASE_TIMEOUT_SECONDS must be a positive integer")
    if release_timeout <= 0:
        sys.exit("KILN_THIRDPARTY_RELEASE_TIMEOUT_SECONDS must be a positive integer")
    if wait_owner_pid_str and not wait_owner_pid_str.isdecimal():
        sys.exit("--wait-owner-pid must be a positive process id")
    wait_owner_pid = int(wait_owner_pid_str) if wait_owner_pid_str else None
    if wait_owner_pid is not None and wait_owner_pid <= 0:
        sys.exit("--wait-owner-pid must be a positive process id")

    def owner_is_alive(pid):
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        except PermissionError:
            return True
        except OSError as exc:
            if exc.errno == errno.ESRCH:
                return False
            if exc.errno == errno.EPERM:
                return True
            raise
        return True

    deadline = barrier_started + release_timeout
    while not os.path.exists(wait_for_release):
        if wait_owner_pid is not None and not owner_is_alive(wait_owner_pid):
            sys.exit(f"gate owner pid {wait_owner_pid} exited before writing the release marker")
        if time.perf_counter() >= deadline:
            if wait_owner_pid is None:
                sys.exit("timed out waiting for gate release marker")
            sys.exit(
                f"timed out after {release_timeout}s waiting for gate release marker "
                f"while gate owner pid {wait_owner_pid} remained alive"
            )
        time.sleep(0.1)
    try:
        with open(wait_for_release, encoding="utf-8") as marker:
            release_state = marker.read().strip()
    except OSError as exc:
        sys.exit(f"cannot read gate release marker: {exc}")
    barrier_elapsed = time.perf_counter() - barrier_started
    print(f"gate barrier wait completed in {barrier_elapsed:.2f}s ({release_state or 'empty'})", flush=True)
    if release_state == "abort":
        sys.exit("gate aborted before releasing thirdparty timing")
    if release_state != "measure":
        sys.exit(f"gate did not release thirdparty timing (marker state: {release_state or 'empty'})")

measurement_started = time.perf_counter()
time.sleep(0.5)

for index, name in enumerate(bundle_names, 1):
    bin_path, err_msg, _build_elapsed_ms = build_results.get(name, (None, "unknown", 0.0))
    if err_msg == "absent":
        print(f"[{index}/{len(bundle_names)}] {name:16} (bundle absent on disk; skipped)", flush=True)
        continue
    if not bin_path:
        message = f"{name}: native compilation failed ({err_msg})"
        regressions.append(message)
        measurement_failures.append(message)
        print(f"[{index}/{len(bundle_names)}] {name:16} COMPILE FAILED ({err_msg})", flush=True)
        continue

    # Validate the exact profile being timed, independently of the dev-profile parity lane.
    # A release build that computes different bytes is not a performance measurement.
    oracle_env = profile_env.copy()
    oracle_env["TZ"] = "UTC"
    bundle_path = os.path.join(bundles_dir, f"{name}.bundle.js")
    oracle = subprocess.run(["node", bundle_path], capture_output=True, env=oracle_env)
    expected_result = (oracle.returncode, oracle.stdout, oracle.stderr)
    def describe_result(result):
        def excerpt(stream):
            text = stream.decode("utf-8", errors="replace")
            return repr(text[:240]) + ("…" if len(text) > 240 else "")
        return (
            f"exit={result.returncode}, stdout={excerpt(result.stdout)}, "
            f"stderr={excerpt(result.stderr)}"
        )

    if oracle.returncode != 0:
        message = (
            f"{name}: Node oracle exited {oracle.returncode}; refusing performance measurement "
            f"({describe_result(oracle)})"
        )
        regressions.append(message)
        measurement_failures.append(message)
        print(f"[{index}/{len(bundle_names)}] {name:16} NODE ORACLE FAILED", flush=True)
        continue

    release = subprocess.run([bin_path], capture_output=True, env=oracle_env)
    if (release.returncode, release.stdout, release.stderr) != expected_result:
        message = (
            f"{name}: native {profile} output differs from Node oracle "
            f"(Node: {describe_result(oracle)}; native: {describe_result(release)})"
        )
        regressions.append(message)
        measurement_failures.append(message)
        print(f"[{index}/{len(bundle_names)}] {name:16} PROFILE VALIDATION FAILED ({message})", flush=True)
        continue

    # Warmup passes to avoid page-fault noise on fresh binaries
    failed = False
    for _ in range(max(2, warmup)):
        proc = subprocess.run([bin_path], capture_output=True, env=oracle_env)
        if (proc.returncode, proc.stdout, proc.stderr) != expected_result:
            message = (
                f"{name}: native warmup output differed from Node oracle "
                f"(Node: {describe_result(oracle)}; native: {describe_result(proc)})"
            )
            regressions.append(message)
            measurement_failures.append(message)
            print(f"[{index}/{len(bundle_names)}] {name:16} WARMUP VALIDATION FAILED ({message})", flush=True)
            failed = True
            break

    if failed:
        continue

    # Execute passes
    times = []
    for _ in range(passes):
        t0 = time.perf_counter()
        proc = subprocess.run([bin_path], capture_output=True, env=oracle_env)
        t1 = time.perf_counter()
        if (proc.returncode, proc.stdout, proc.stderr) != expected_result:
            message = (
                f"{name}: native timed output differed from Node oracle "
                f"(Node: {describe_result(oracle)}; native: {describe_result(proc)})"
            )
            regressions.append(message)
            measurement_failures.append(message)
            print(f"[{index}/{len(bundle_names)}] {name:16} TIMED VALIDATION FAILED ({message})", flush=True)
            failed = True
            break
        times.append((t1 - t0) * 1000.0)

    if failed or not times:
        continue

    median_ms = round(statistics.median(times), 2)
    min_ms = round(min(times), 2)
    measured[name] = {
        "median_ms": median_ms,
        "min_ms": min_ms,
        "workload_sha256": file_sha256(os.path.join(bundles_dir, f"{name}.bundle.js")),
    }

    was = platform_record.get("bundles", {}).get(name) if platform_record else None
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

measurement_elapsed = time.perf_counter() - measurement_started
print(f"serial warmup/measurement phase completed in {measurement_elapsed:.2f}s", flush=True)

if update:
    if measurement_failures:
        sys.exit(
            "refusing to update thirdparty baseline because measurements failed: "
            + "; ".join(measurement_failures)
        )
    if not measured:
        sys.exit("refusing to update thirdparty baseline because no bundle was measured")

    if baseline.get("schema") == 1:
        # Preserve an old unqualified file for audit, but never treat its entries as comparable.
        baseline = {
            "schema": 2,
            "threshold_pct": threshold,
            "platforms": {},
            "legacy_unqualified": baseline,
        }
    baseline["schema"] = 2
    baseline["threshold_pct"] = threshold
    baseline.setdefault("platforms", {})
    if platform_record is None:
        platform_record = {
            "platform": platform_details,
            "profile": profile,
            "bundles": {},
        }
        baseline["platforms"][platform_key] = platform_record
    platform_record["platform"] = platform_details
    platform_record["profile"] = profile
    run_provenance = {
        "recorded_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "source_commit": git_output("rev-parse", "HEAD"),
        "source_diff_sha256": hashlib.sha256((git_output("diff", "--binary", "HEAD") or "").encode()).hexdigest(),
        "working_tree_dirty": bool(git_output("status", "--porcelain")),
        "kiln_binary_sha256": file_sha256(kiln) if os.path.isfile(kiln) else None,
        "rustc_version": tool_version(os.environ.get("RUSTC", "rustc")),
        "clang_version": tool_version(os.environ.get("CC", "clang")),
        "profile": profile,
        "passes": passes,
        "warmup": warmup,
        "threshold_pct": threshold,
        "min_delta_ms": min_delta_ms,
    }
    bundles = platform_record.setdefault("bundles", {})
    previous_provenance = platform_record.get("provenance")
    for bundle_record in bundles.values():
        if "measurement_provenance" not in bundle_record:
            # Older schema-2 records only had one platform-wide provenance object. Preserve
            # its scope before replacing it with the provenance for this partial update.
            bundle_record["measurement_provenance"] = previous_provenance
    for name, bundle_record in measured.items():
        bundle_record["measurement_provenance"] = run_provenance
    bundles.update(measured)
    platform_record["provenance"] = {
        "scope": "latest_update",
        "updated_bundles": sorted(measured),
        **run_provenance,
    }
    os.makedirs(os.path.dirname(os.path.abspath(baseline_path)), exist_ok=True)
    with open(baseline_path, "w") as handle:
        json.dump(baseline, handle, indent=2, sort_keys=True)
        handle.write("\n")
    print(f"\nwrote {baseline_path}: platform={platform_key}, {len(platform_record['bundles'])} bundle(s) recorded")
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
	# Capture both streams from the SAME execution. Running once per stream doubles compilation
	# and execution, and can pair stdout with a different run's stderr and exit status.
	local expected expected_err actual actual_err
	local expected_status=0 actual_status=0
	TZ=UTC node "$bundle" >"$WORK_DIR/$name.node.stdout" 2>"$WORK_DIR/$name.node.stderr" || expected_status=$?
	printf '%s\n' "$expected_status" >"$WORK_DIR/$name.node.status"
	"$KILN" --one-shot --clock virtual ${adapter:+"$adapter"} run "$bundle" >"$WORK_DIR/$name.interp.stdout" 2>"$WORK_DIR/$name.interp.stderr" || actual_status=$?
	expected="$(<"$WORK_DIR/$name.node.stdout")"
	expected_err="$(<"$WORK_DIR/$name.node.stderr")"
	actual="$(<"$WORK_DIR/$name.interp.stdout")"
	actual_err="$(<"$WORK_DIR/$name.interp.stderr")"
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
	# The interpreted pass already ran the Node oracle for this bundle.
	expected_status="$(<"$WORK_DIR/$name.node.status")"
	expected="$(<"$WORK_DIR/$name.node.stdout")"
	expected_err="$(<"$WORK_DIR/$name.node.stderr")"
	"$KILN" --one-shot --clock virtual ${adapter:+"$adapter"} run --native "$bundle" >"$WORK_DIR/$name.native.stdout" 2>"$WORK_DIR/$name.native.stderr" || actual_status=$?
	actual="$(<"$WORK_DIR/$name.native.stdout")"
	actual_err="$(<"$WORK_DIR/$name.native.stderr")"
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
