#!/usr/bin/env bash
#
# Every microbenchmark under bench/, under every runtime, in one table.
#
# The numbers this replaces were wrong in a way that flattered us. They timed whole processes, so
# node's 45ms of startup sat inside every row, and they timed each workload once, so node and bun
# were measured before their JITs had warmed up. Warm, `array-functional` is 11x slower under the
# native backend than under node; cold, it looked like 1.3x. See bench/harness.mjs.
#
# So the harness times from inside the process and reports a warm steady state. This script's job
# is the part that cannot be done from in there: build the native binaries, drive every runtime,
# and refuse to print a number it cannot trust.
#
# Two refusals matter. A runtime whose checksum differs from node's did different work, and a
# fast wrong answer reads exactly like a win -- it is reported as a divergence and never as a
# time. And the interpreter cannot time itself at all (its realm clock is virtual, ADR 0044), so
# it reports iterations instead and is timed here, against a floor measured the same way.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR/..
# shellcheck source=../scripts/lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/../scripts/lib.sh"
cd "$(kiln_repo_root)"

KILN_BIN="${KILN_BENCH_KILN:-target/release/kiln}"
[[ -x $KILN_BIN ]] || {
	echo "bench/compare.sh: no kiln binary at $KILN_BIN (build with: mbx build --release -p kiln-cli)" >&2
	exit 1
}
for runtime in node bun; do
	command -v "$runtime" >/dev/null || {
		echo "bench/compare.sh: $runtime is not on PATH" >&2
		exit 1
	}
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/kiln-bench.XXXXXXXX")"
trap 'rm -rf "$WORK"' EXIT
mkdir -p .scratch

python3 - "$KILN_BIN" "$WORK" "${1:-}" <<'PY'
import json, os, statistics, subprocess, sys, time
from bench.checksum_validation import validate_repeated_checksums

kiln, work, out_path = sys.argv[1], sys.argv[2], sys.argv[3]
PASSES = int(os.environ.get("PASSES", "3"))

scripts = []
for suite in ("cpu", "non-io", "io"):
    directory = os.path.join("bench", suite)
    for name in sorted(os.listdir(directory)):
        if name.endswith(".mjs"):
            scripts.append((suite, name, os.path.join(directory, name)))

def node_surface(path):
    """Whether this benchmark reaches for the Node adapter kiln installs behind --node."""
    with open(path) as handle:
        source = handle.read()
    return "node:" in source or "Buffer" in source

def argv_for(runtime, path, native_binary):
    if runtime == "node":
        return ["node", path]
    if runtime == "bun":
        return ["bun", "run", path]
    if runtime == "kiln":
        flags = ["--node"] if node_surface(path) else []
        return [kiln, "--one-shot", "--clock", "virtual", *flags, "run", path]
    return [native_binary]

def run(argv):
    """Time one process and pull the harness's JSON line out of its output.

    Bytes rather than text: a runtime is free to put anything on stdout, and one of them puts
    bytes that are not UTF-8 there. Decoding strictly made that an exception in the *runner*
    half an hour into a run, which reports a decoding accident as if the benchmark had failed.
    """
    started = time.perf_counter()
    done = subprocess.run(argv, capture_output=True)
    wall = (time.perf_counter() - started) * 1000.0
    stdout = done.stdout.decode("utf-8", errors="replace")
    stderr = done.stderr.decode("utf-8", errors="replace")
    record = None
    for line in stdout.splitlines():
        line = line.strip()
        if line.startswith("{"):
            try:
                record = json.loads(line)
            except ValueError:
                pass
    return wall, record, done.returncode, stderr.strip()

# Native binaries are built once and outside every measurement: a C compile is not what this
# compares, and charging it per run would report the backend as slower than the compiler that
# produced it by two orders of magnitude.
native = {}
print("building native binaries", file=sys.stderr)
for suite, name, path in scripts:
    binary = os.path.join(work, f"{suite}-{name}.bin")
    argv = [kiln, "--one-shot"]
    if node_surface(path):
        argv.append("--node")
    argv += ["build", path, "--profile", "native-release", "--output", binary]
    built = subprocess.run(argv, capture_output=True, text=True)
    native[path] = binary if built.returncode == 0 and os.path.exists(binary) else None
    if native[path] is None:
        print(f"  native build refused: {suite}/{name}", file=sys.stderr)

# The floor each runtime pays before any of the work: `bench/floor.mjs`, an empty workload run
# through the same harness, so the floor includes the clock probe as well as process start.
#
# kiln is measured twice, because `--node` changes what it pays *and* which clock it gets: the
# adapter installs a deterministic host clock, so a benchmark that needs `--node` takes a
# different path through the harness than one that does not. Subtracting the wrong floor from an
# externally timed row is how a measurement quietly acquires a constant.
empty = os.path.join("bench", "floor.mjs")
empty_binary = os.path.join(work, "floor.bin")
subprocess.run([kiln, "--one-shot", "build", empty, "--profile", "native-release",
                "--output", empty_binary], capture_output=True)
RUNTIMES = ["node", "bun", "kiln", "kiln-native"]
floor = {}
for runtime in RUNTIMES:
    binary = empty_binary if runtime == "kiln-native" else None
    times = [run(argv_for(runtime, empty, binary))[0] for _ in range(PASSES)]
    floor[runtime] = statistics.median(times)
adapter_argv = [kiln, "--one-shot", "--clock", "virtual", "--node", "run", empty]
floor["kiln+node"] = statistics.median([run(adapter_argv)[0] for _ in range(PASSES)])

rows, divergences = [], []
for suite, name, path in scripts:
    measured, checksums = {}, {}
    for runtime in RUNTIMES:
        binary = native.get(path)
        if runtime == "kiln-native" and binary is None:
            divergences.append(f"{suite}/{name} {runtime}: native build refused")
            continue
        samples = []
        for _ in range(PASSES):
            wall, record, code, stderr = run(argv_for(runtime, path, binary))
            if code != 0 or record is None:
                divergences.append(f"{suite}/{name} {runtime}: exit {code} {stderr[:80]}")
                samples = []
                break
            # A record whose keys are not the ones the harness wrote is not a slow result, it
            # is a corrupt one: the native backend can return an object literal with mangled
            # keys (`"samples"` as `"samplesle"`, `"checksum"` with raw bytes spliced into it).
            # Reading it with `[]` made that a KeyError in the runner, which reports a wrong
            # answer as a broken harness.
            required = {"mode", "checksum"} | (
                {"warm_median"} if record.get("mode") == "in-process" else {"iterations"})
            if not required <= record.keys():
                divergences.append(
                    f"{suite}/{name} {runtime}: corrupt record, keys {sorted(record)!r}")
                samples = []
                break
            checksums.setdefault(runtime, []).append(record["checksum"])
            if record["mode"] == "in-process":
                samples.append(record["warm_median"])
            else:
                # Timed from out here, because in there the clock does not move -- or moves
                # per read, which is worse. The floor is the one this exact invocation pays,
                # which for the interpreter depends on whether `--node` was passed.
                paid = floor["kiln+node"] if runtime == "kiln" and node_surface(path) else floor[runtime]
                samples.append(max(wall - paid, 0.0) / record.get("iterations", 1))
        if samples:
            measured[runtime] = statistics.median(samples)
    excluded, checksum_diagnostics = validate_repeated_checksums(checksums, PASSES)
    divergences.extend(f"{suite}/{name} {diagnostic}" for diagnostic in checksum_diagnostics)
    for runtime in excluded:
        measured.pop(runtime, None)
    rows.append({"suite": suite, "script": name, "ms": measured})

label = {"node": "node", "bun": "bun", "kiln": "kiln", "kiln-native": "native"}
print(f"\nfloor (ms): " + "  ".join(f"{label[r]} {floor[r]:.1f}" for r in RUNTIMES)
      + f"  kiln+node {floor['kiln+node']:.1f}")
print("warm steady state, ms per iteration; kiln is the interpreter, native the compiled backend\n")
print(f"{'suite/script':30s} {'node':>9s} {'bun':>9s} {'kiln':>10s} {'native':>10s}   {'native/node':>11s}")
for row in rows:
    m = row["ms"]
    # Three significant figures rather than two decimals: a warm iteration of `frame` is under a
    # microsecond, and `0.00` is not a measurement.
    def cell(runtime, width):
        if runtime not in m:
            return f"{'--':>{width}s}"
        value = m[runtime]
        return f"{value:{width}.2f}" if value >= 1 else f"{value:{width}.4f}"
    ratio = f"{m['kiln-native'] / m['node']:.2f}x" if {"kiln-native", "node"} <= m.keys() else "--"
    print(f"{row['suite'] + '/' + row['script']:30s} {cell('node', 9)} {cell('bun', 9)} "
          f"{cell('kiln', 10)} {cell('kiln-native', 10)}   {ratio:>11s}")

if divergences:
    print("\ndivergences (excluded from the table above):", file=sys.stderr)
    for line in divergences:
        print(f"  {line}", file=sys.stderr)

if out_path:
    with open(out_path, "w") as handle:
        json.dump({"floor_ms": floor, "passes": PASSES, "rows": rows,
                   "divergences": divergences}, handle, indent=2)
        handle.write("\n")
    print(f"\nwrote {out_path}")
PY
