#!/usr/bin/env bash
#
# Does the TypeScript checker's output change the native code? The measurement behind
# docs/issues/type-directed-codegen/PRD.md.
#
# For each typed twin (bench/**/<name>.ts beside <name>.mjs) and bench/realworld/programs/json-transform.ts
# this builds natively
#   * the untyped .mjs original,
#   * the typed twin with the checker's facts reaching codegen,
#   * the same typed twin with KILN_SOURCE_TYPES=0 (facts withheld),
# runs each binary PASSES times, takes the median of the harness's `warm_median`, refuses a row
# whose checksums disagree (a fast wrong answer reads exactly like a win), and counts the
# speculations the emitter made, by evidence (KILN_NATIVE_TRACE_SPECULATION=1).
#
# Timing refuses to start when the 1-minute load average is above 1.5 -- a number taken on a busy
# machine is a clock reading -- unless --force is given.
#
# usage: scripts/typed-codegen.sh [--force] [--passes N] [--only NAME[,NAME]] [--out FILE.md]
set -euo pipefail

# shellcheck source-path=SCRIPTDIR/..
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
ROOT="$(kiln_repo_root)"
cd "$ROOT"

FORCE=0
PASSES=5
ONLY=""
OUT=""
while (($#)); do
	case $1 in
	--force) FORCE=1 ;;
	--passes) PASSES=$2; shift ;;
	--only) ONLY=$2; shift ;;
	--out) OUT=$2; shift ;;
	-h | --help) sed -n '2,22p' "$0"; exit 0 ;;
	*) echo "typed-codegen.sh: unknown argument $1" >&2; exit 2 ;;
	esac
	shift
done

KILN_BIN="${KILN_BENCH_KILN:-target/release/kiln}"
[[ -x $KILN_BIN ]] || {
	echo "typed-codegen.sh: no kiln binary at $KILN_BIN (build with: mbx build --release -p kiln-cli)" >&2
	exit 1
}

LOAD_LIMIT=1.5
load_now() { cut -d' ' -f1 /proc/loadavg 2>/dev/null || uptime | sed 's/.*load averages*: *//; s/[ ,].*//'; }
load_ok() { awk -v l="$(load_now)" -v m="$LOAD_LIMIT" 'BEGIN { exit !(l <= m) }'; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/kiln-typed-codegen.XXXXXXXX")"
# Trace builds run on private copies of bench/ inside the checkout (so the project root, and the
# checker's view of it, is the repository's) under .scratch, which a trap removes.
TRACE_BASE="$ROOT/.scratch/typed-codegen.$$"
mkdir -p "$TRACE_BASE"
trap 'rm -rf "$WORK" "$TRACE_BASE"' EXIT

export TRACE_BASE WORK PASSES ONLY FORCE KILN_BIN LOAD_LIMIT
export LOAD_AT_START="$(load_now)"
export KILN_COMMIT="$(git rev-parse --short HEAD)"

python3 - "$OUT" <<'PY'
import json, os, re, statistics, subprocess, sys, shutil, time

out_path = sys.argv[1]
kiln = os.environ["KILN_BIN"]
work = os.environ["WORK"]
trace_base = os.environ["TRACE_BASE"]
passes = int(os.environ["PASSES"])
force = os.environ["FORCE"] == "1"
only = [n for n in os.environ["ONLY"].split(",") if n]
limit = float(os.environ["LOAD_LIMIT"])

PROGRAMS = [
    ("nbody", "bench/cpu/nbody"),
    ("mandelbrot", "bench/cpu/mandelbrot"),
    ("class-poly", "bench/non-io/class-poly"),
    ("object-literal", "bench/non-io/object-literal"),
    ("string-ops", "bench/non-io/string-ops"),
    ("array-functional", "bench/non-io/array-functional"),
    ("array-param", "bench/non-io/array-param"),
]
if only:
    PROGRAMS = [p for p in PROGRAMS if p[0] in only]

def load():
    return os.getloadavg()[0]

def build(source, binary, env_extra=None, cwd=None, trace=False):
    env = dict(os.environ)
    env.update(env_extra or {})
    if trace:
        env["KILN_NATIVE_TRACE_SPECULATION"] = "1"
    done = subprocess.run(
        [os.path.abspath(kiln), "--one-shot", "build", source, "--profile", "native-release",
         "--output", binary],
        capture_output=True, text=True, env=env, cwd=cwd)
    if done.returncode != 0:
        return None, done.stderr.strip().splitlines()[-1:] or ["build failed"]
    return done.stderr, None

def trace_lines(stderr):
    """One normalised record per function line: everything but the function index and symbol."""
    records = []
    for line in (stderr or "").splitlines():
        if line.startswith("kiln-speculation "):
            tokens = [t for t in line.split()[1:] if not t.startswith(("fn=", "symbol="))]
            records.append(" ".join(tokens))
    return records

def speculation_counts(records, baseline):
    """Counts of speculations by evidence, from the emitter's own trace lines.

    `baseline` holds the lines of an empty benchmark (prelude and harness), subtracted as a
    multiset so only the program's own functions are counted.
    """
    remaining = {}
    for record in baseline:
        remaining[record] = remaining.get(record, 0) + 1
    counts = {}
    def bump(key, by=1):
        counts[key] = counts.get(key, 0) + by
    for record in records:
        if remaining.get(record, 0) > 0:
            remaining[record] -= 1
            continue
        for token in record.split():
            m = re.match(r"(number_param|array_param)\[\d+\]=(\w+)", token)
            if m:
                bump(f"{m.group(1)}:{m.group(2)}")
                continue
            m = re.match(r"(spec_arrays|spec_calls|spec_fields|spread_sinks)=(\d+)", token)
            if m:
                bump(m.group(1), int(m.group(2)))
            elif token.startswith("typed_entry="):
                bump("typed_entry")
            elif token.startswith("typed_return="):
                bump("typed_return")
            elif token == "versioned=false":
                bump("unversioned_fns")
            elif token == "source_types=absent":
                bump("fns_without_facts")
    return counts

def fmt_counts(counts):
    if not counts:
        return "none"
    return ", ".join(f"{k}={v}" for k, v in sorted(counts.items()))

def run_once(binary, whole=False):
    started = time.perf_counter()
    done = subprocess.run([binary], capture_output=True)
    if whole:
        # No harness: the program prints its answer; time the process and use stdout as checksum.
        return {"warm_median": (time.perf_counter() - started) * 1000.0,
                "checksum": done.stdout.decode("utf-8", "replace")}
    for line in reversed(done.stdout.decode("utf-8", "replace").splitlines()):
        line = line.strip()
        if line.startswith("{") and "warm_median" in line:
            return json.loads(line)
    raise RuntimeError(f"{binary}: no harness line in output")

def measure(binary, whole=False):
    runs = [run_once(binary, whole) for _ in range(passes)]
    return statistics.median(r["warm_median"] for r in runs), {r["checksum"] for r in runs}

# The trace of an empty benchmark: prelude and harness functions, not the program's own.
baseline_dir = os.path.join(trace_base, "_empty")
os.makedirs(baseline_dir)
shutil.copytree("bench", os.path.join(baseline_dir, "bench"),
                ignore=shutil.ignore_patterns("results", "node_modules", "inputs", "uglify-capture", "*.py"))
with open(os.path.join(baseline_dir, "bench", "cpu", "__empty.mjs"), "w") as f:
    f.write('import { bench } from "../harness.mjs";\nbench("empty", () => 0);\n')
base_stderr, base_err = build(os.path.join(baseline_dir, "bench", "cpu", "__empty.mjs"),
                              os.path.join(baseline_dir, "out"), {}, cwd=baseline_dir, trace=True)
baseline = trace_lines(base_stderr)

# Variants: label -> (source suffix, env)
rows = []
refused = []
for name, base in PROGRAMS + [("json-transform", "bench/realworld/programs/json-transform")]:
    real = name == "json-transform"
    ts = base + ".ts"
    mjs = None if real else base + ".mjs"
    variants = {}
    if mjs:
        variants["untyped"] = (mjs, {})
    variants["typed"] = (ts, {})
    variants["withheld"] = (ts, {"KILN_SOURCE_TYPES": "0"})
    bins, specs = {}, {}
    for label, (src, env) in variants.items():
        binary = os.path.join(work, f"{name}.{label}")
        # Timing binaries build in place (and may hit a warm cache); the trace build below runs in
        # a private copy of bench/ so the emitter always runs -- a cache hit prints no trace.
        stderr, err = build(src, binary, env, trace=False)
        if err:
            refused.append(f"{name} {label}: {err}")
            continue
        bins[label] = binary
        # Second build, fresh copy, for the trace.
        copy = os.path.join(trace_base, name, label)
        if os.path.exists(copy):
            shutil.rmtree(copy)
        os.makedirs(copy)
        shutil.copytree("bench", os.path.join(copy, "bench"),
                        ignore=shutil.ignore_patterns("results", "node_modules", "inputs", "uglify-capture", "*.py"))
        tr_stderr, tr_err = build(os.path.join(copy, src), os.path.join(copy, "out"), env,
                                  cwd=copy, trace=True)
        specs[label] = (speculation_counts(trace_lines(tr_stderr), baseline)
                        if not tr_err else {"trace_failed": 1})
    rows.append((name, real, bins, specs))

# Timing, behind the load gate.
def wait_for_quiet():
    if force:
        return True
    for _ in range(60):
        if load() <= limit:
            return True
        time.sleep(10)
    return False

if not force and load() > limit:
    print(f"typed-codegen.sh: load average {load():.2f} > {limit}; refusing to time "
          f"(waiting up to 10 minutes; pass --force to override)", file=sys.stderr)
    if not wait_for_quiet():
        print(f"typed-codegen.sh: still {load():.2f} after waiting; not timing", file=sys.stderr)
        sys.exit(3)

load_before = load()
table = []
divergences = []
for name, real, bins, specs in rows:
    result = {}
    sums = {}
    for label, binary in bins.items():
        if not force and load() > limit * 1.5:
            wait_for_quiet()
        ms, checks = measure(binary, real)
        result[label] = ms
        sums[label] = checks
    allsums = set().union(*sums.values()) if sums else set()
    ok = len(allsums) == 1
    if not ok:
        divergences.append(f"{name}: checksums {sums}")
    table.append((name, real, result, specs, ok))
load_after = load()

def cell(v):
    return "n/a" if v is None else f"{v:.3f}"

lines = []
lines.append("| program | untyped .mjs (ms) | typed, types used (ms) | typed, types withheld (ms) | typed/withheld | typed vs untyped | checksum |")
lines.append("| --- | --- | --- | --- | --- | --- | --- |")
for name, real, r, specs, ok in table:
    u, t, w = r.get("untyped"), r.get("typed"), r.get("withheld")
    ratio = (lambda a, b: f"{a / b:.2f}x" if a is not None and b else "n/a")
    lines.append(f"| {name} | {cell(u)} | {cell(t)} | {cell(w)} | {ratio(t, w)} | "
                 f"{ratio(t, u)} | {'agree' if ok else 'DIVERGE'} |")
lines.append("")
lines.append("Speculations the emitter made, by evidence (`KILN_NATIVE_TRACE_SPECULATION=1`):")
lines.append("")
lines.append("| program | untyped .mjs | typed, types used | typed, types withheld |")
lines.append("| --- | --- | --- | --- |")
for name, real, r, specs, ok in table:
    lines.append(f"| {name} | {fmt_counts(specs.get('untyped')) if not real else 'n/a'} | "
                 f"{fmt_counts(specs.get('typed'))} | {fmt_counts(specs.get('withheld'))} |")
text = "\n".join(lines)
header = (f"load average 1m: {load_before:.2f} before timing, {load_after:.2f} after; "
          f"{passes} passes per binary, median of the harness's warm_median "
          f"(json-transform has no .mjs original and reports whole-run time)")
print(header)
print()
print(text)
if refused:
    print("\nbuilds refused:\n  " + "\n  ".join(refused))
if divergences:
    print("\nCHECKSUM DIVERGENCE:\n  " + "\n  ".join(divergences))
if out_path:
    with open(out_path, "w") as f:
        f.write(header + "\n\n" + text + "\n")
sys.exit(1 if divergences else 0)
PY
