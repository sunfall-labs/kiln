#!/usr/bin/env bash
#
# What a ten-line HTTP server costs to ship, measured across the runtimes kiln means to compete
# with, and recorded before kiln can compete.
#
# This is not kiln-bench. That harness measures design.md §13's iteration loops through
# kiln-db's QueryEngine, and everything in it is kiln's own work. This measures the *shipped
# artifact* instead - cold start, peak RSS, and bytes on disk - for other people's toolchains,
# on a program kiln cannot yet run at all. Two different axes, two different tools; putting this
# inside kiln-bench would have coupled a regression gate to whether deno happened to be
# installed.
#
# WHY RECORD IT NOW. README.md's priority list puts native runtime performance fourth, and the
# distinguishing claim for the compiled-server target is not "faster than V8" - it is that
# `deno compile` and `bun build --compile` put an entire JavaScript engine inside the executable
# and kiln does not. That claim is worth what its measurement is worth, and a measurement taken
# after kiln can run the program is a measurement taken by someone who already knows what answer
# they want. So the baseline column is recorded first, from a checkout where kiln scores nothing.
#
# HONESTY RULES, the same ones conformance/status.sh and kiln_report_has_measurements enforce:
#
#   - A runtime that is not installed is recorded `"available": false` with a reason. It is never
#     a zero, and never absent from the document: a missing row reads as "not applicable" and an
#     absent runtime is nothing of the kind.
#   - kiln is measured too, on the same program, and its refusal is recorded verbatim. The column
#     is empty because the compiler said something specific, and the document says what.
#   - Nothing here is a gate. There is no threshold and no exit-1-on-regression, because these
#     numbers are properties of node, deno and bun rather than of this repository, and failing
#     our CI over someone else's release would be theatre.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

PROBE="conformance/server/s01-http-hello.mjs"
OUT="bench/results/server-baseline.json"
ITERATIONS=10
WARMUP=2

while [[ $# -gt 0 ]]; do
	case "$1" in
	--out)
		OUT="${2:?--out needs a path}"
		shift 2
		;;
	--iterations)
		ITERATIONS="${2:?--iterations needs a number}"
		shift 2
		;;
	--probe)
		PROBE="${2:?--probe needs a path}"
		shift 2
		;;
	-h | --help)
		cat <<-'USAGE'
			usage: scripts/server-baseline.sh [options]

			  --out <file>        where to write the JSON document
			                      (default: bench/results/server-baseline.json)
			  --iterations <n>    measured runs per target after warmup (default: 10)
			  --probe <file>      the program to measure (default: conformance/server/s01-http-hello.mjs)

			Measures cold start, peak RSS and artifact size for the same self-driving HTTP server
			under node, deno and bun - each both as a script and, where the toolchain offers it,
			as a standalone executable - and records what kiln currently says about the same file.

			Never gates. See the header of this file for why.
		USAGE
		exit 0
		;;
	*)
		echo "server-baseline.sh: unknown argument '$1' (try --help)" >&2
		exit 2
		;;
	esac
done

if [[ ! -f $PROBE ]]; then
	echo "server-baseline.sh: no probe at $PROBE" >&2
	exit 2
fi

KILN_BIN="target/debug/kiln"
[[ -x $KILN_BIN ]] || KILN_BIN=""

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$(dirname -- "$OUT")"

# The measurement itself is Python because the two numbers that matter are awkward from the
# shell. `os.wait4` returns the rusage of *one specific child*, so peak RSS needs no parsing of
# /usr/bin/time and no guessing at whether this platform reports kilobytes or bytes; and
# perf_counter is monotonic, which `date` on macOS cannot even be asked for. lib.sh already
# reaches for python3 for exactly this class of job.
python3 - "$PROBE" "$WORK" "$ITERATIONS" "$WARMUP" "${KILN_BIN}" >"$WORK/report.json" <<'PY'
import json
import os
import shutil
import statistics
import subprocess
import sys
import time

probe, work, iterations, warmup, kiln_bin = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), sys.argv[5]

# ru_maxrss is bytes on Darwin and kilobytes on Linux. Getting this backwards would be a
# thousandfold error in the headline number, so it is decided once, explicitly, and recorded in
# the document rather than assumed by whoever reads it.
RSS_SCALE = 1 if sys.platform == "darwin" else 1024

EXPECTED = "200 text/plain kiln\n"


def measure(argv):
    """Wall time and peak RSS of one run, or the reason there is none.

    The output is checked against what the probe is supposed to print. A runtime that starts
    fast because it crashed early would otherwise post the best number in the table.
    """
    samples, rss = [], []
    for index in range(warmup + iterations):
        read_fd, write_fd = os.pipe()
        started = time.perf_counter()
        pid = os.posix_spawn(argv[0], argv, os.environ, file_actions=[
            (os.POSIX_SPAWN_CLOSE, read_fd),
            (os.POSIX_SPAWN_DUP2, write_fd, 1),
            (os.POSIX_SPAWN_DUP2, write_fd, 2),
        ])
        os.close(write_fd)
        with os.fdopen(read_fd, "rb") as handle:
            output = handle.read()
        _, status, usage = os.wait4(pid, 0)
        elapsed = (time.perf_counter() - started) * 1000.0
        if status != 0:
            return None, "exited %d: %s" % (status, output.decode("utf-8", "replace").strip()[:200])
        if output.decode("utf-8", "replace") != EXPECTED:
            return None, "printed %r, not the probe's recorded output" % output.decode("utf-8", "replace")[:200]
        if index >= warmup:
            samples.append(elapsed)
            rss.append(usage.ru_maxrss * RSS_SCALE)
    return {
        "startup_ms_median": round(statistics.median(samples), 2),
        "startup_ms_min": round(min(samples), 2),
        "peak_rss_bytes": int(statistics.median(rss)),
    }, None


def which(name):
    return shutil.which(name)


def build(argv, artifact):
    """Run a compile step, returning the artifact size or the reason there is none."""
    result = subprocess.run(argv, capture_output=True, text=True)
    if result.returncode != 0:
        return None, (result.stderr or result.stdout).strip()[:300]
    if not os.path.exists(artifact):
        return None, "the compile step reported success and produced no file at %s" % artifact
    return os.path.getsize(artifact), None


targets = []


def script_target(name, binary, argv_extra, note):
    path = which(binary)
    if path is None:
        targets.append({
            "target": name, "form": "script", "available": False,
            "reason": "%s is not on PATH" % binary,
        })
        return
    metrics, failure = measure([path] + argv_extra + [os.path.abspath(probe)])
    entry = {"target": name, "form": "script", "available": failure is None, "runtime_path": path, "note": note}
    if failure is None:
        entry.update(metrics)
    else:
        entry["reason"] = failure
    targets.append(entry)


def standalone_target(name, binary, compile_argv, artifact, note):
    path = which(binary)
    if path is None:
        targets.append({
            "target": name, "form": "standalone", "available": False,
            "reason": "%s is not on PATH" % binary,
        })
        return
    size, failure = build([path] + compile_argv, artifact)
    if failure is not None:
        targets.append({"target": name, "form": "standalone", "available": False, "reason": failure})
        return
    metrics, failure = measure([artifact])
    entry = {
        "target": name, "form": "standalone", "available": failure is None,
        "artifact_bytes": size, "note": note,
    }
    if failure is None:
        entry.update(metrics)
    else:
        entry["reason"] = failure
    targets.append(entry)


script_target("node", "node", [], "no standalone form measured: Node's SEA is a multi-step "
              "postject flow rather than one command, and half-doing it would put a number in "
              "this table that nobody could reproduce from the line above it")
script_target("deno", "deno", ["run", "--allow-net", "--quiet"], "")
script_target("bun", "bun", ["run"], "")

standalone_target("deno", "deno", [
    "compile", "--allow-net", "--quiet", "--output", os.path.join(work, "s01-deno"),
    os.path.abspath(probe),
], os.path.join(work, "s01-deno"), "the JavaScript engine is inside this file")
standalone_target("bun", "bun", [
    "build", "--compile", "--outfile", os.path.join(work, "s01-bun"), os.path.abspath(probe),
], os.path.join(work, "s01-bun"), "the JavaScript engine is inside this file")

# The column this whole document exists to eventually fill. Recorded as a refusal with its exact
# text, not omitted: "kiln has no number here" and "kiln was never asked" are different facts,
# and only one of them is true.
kiln = {"target": "kiln", "form": "standalone", "available": False}
if not kiln_bin or not os.path.exists(kiln_bin):
    kiln["reason"] = "no kiln binary at target/debug/kiln; build one with `mbx build -p kiln-cli`"
else:
    result = subprocess.run([os.path.abspath(kiln_bin), "--one-shot", "run", probe],
                            capture_output=True, text=True)
    refusal = (result.stderr or result.stdout).strip().splitlines()
    kiln["reason"] = refusal[0] if refusal else "exited %d and said nothing" % result.returncode
    kiln["note"] = ("the target of docs/design.md §14 step 10. Until it runs this file there is "
                    "nothing to measure, and a placeholder number would be a claim")
targets.append(kiln)


def version(binary, argv):
    path = which(binary)
    if path is None:
        return None
    result = subprocess.run([path] + argv, capture_output=True, text=True)
    return (result.stdout or result.stderr).strip().splitlines()[0] if result.returncode == 0 else None


json.dump({
    "schema": 1,
    "probe": probe,
    "iterations": iterations,
    "warmup": warmup,
    "platform": sys.platform,
    "rss_units": "bytes (ru_maxrss scaled by %d for this platform)" % RSS_SCALE,
    "versions": {
        "node": version("node", ["--version"]),
        "deno": version("deno", ["--version"]),
        "bun": version("bun", ["--version"]),
    },
    "targets": targets,
}, sys.stdout, indent=2, sort_keys=False)
sys.stdout.write("\n")
PY

cp "$WORK/report.json" "$OUT"

kiln_say "### Compiled-server baseline"
kiln_say ""
kiln_say "The same program under every runtime: \`$PROBE\`, a self-driving HTTP server that"
kiln_say "answers one request and exits. All three runtimes print the byte-identical line the"
kiln_say "conformance corpus records, so the comparison is over one program and not three."
kiln_say ""
kiln_say '```'
python3 - "$OUT" <<'PY' | kiln_block
import json, sys

doc = json.load(open(sys.argv[1]))


def human(n):
    if n is None:
        return "-"
    if n < 1024:
        return "%d B" % n
    for unit in ("KiB", "MiB", "GiB"):
        n /= 1024.0
        if n < 1024:
            return "%.1f %s" % (n, unit)
    return "%.1f TiB" % n


print("%-8s %-11s %10s %10s %12s" % ("target", "form", "start", "peak RSS", "artifact"))
print("-" * 55)
for entry in doc["targets"]:
    if entry.get("available"):
        print("%-8s %-11s %9.1fms %10s %12s" % (
            entry["target"], entry["form"], entry["startup_ms_median"],
            human(entry.get("peak_rss_bytes")), human(entry.get("artifact_bytes")),
        ))
    else:
        print("%-8s %-11s %s" % (entry["target"], entry["form"], "not measured"))
print()
for entry in doc["targets"]:
    if not entry.get("available"):
        print("%s (%s): %s" % (entry["target"], entry["form"], entry.get("reason", "no reason recorded")))
PY
kiln_say '```'
kiln_say ""
kiln_say "Written to \`$OUT\`. Medians over $ITERATIONS runs after $WARMUP discarded."
kiln_say "This is not a gate and has no threshold: these are properties of node, deno and bun."
