#!/usr/bin/env bash
#
# Check the committed Test262 chunk baseline, or regenerate it with --update.
#
# A whole-corpus number is one figure and hides everything: a change that wins forty rows in one
# chunk while losing forty in another reads as no change at all. This runs the corpus chunk by
# chunk and compares each against `conformance/test262-baseline.json`, so a regression names the
# chunk it happened in.
#
# Regenerating is a deliberate, manual act, for the same reason `update-baseline.sh` says so about
# the performance baseline: moving the numbers shows up as a diff a reviewer has to accept, next to
# the change that caused it. Nothing in CI runs `--update`.
#
# The corpus is *not* fetched here. Point `KILN_TEST262_CHECKOUT` at a checkout, or keep the
# default one at `~/.cache/kiln/test262-<short-commit>`; `scripts/test262.sh` fetches and verifies
# it. Deliberately not under `TMPDIR`: the periodic cleaner empties that on a day boundary and took
# a whole checkout mid-session once, and an emptied corpus reports `pass 0 fail 0` and exits 0,
# which reads exactly like a clean run.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

readonly TEST262_COMMIT="771005236e88a909635104e03ba12559688c0172"
readonly BASELINE="conformance/test262-baseline.json"
CHECKOUT="${KILN_TEST262_CHECKOUT:-$HOME/.cache/kiln/test262-${TEST262_COMMIT:0:10}}"
UPDATE=0
[[ ${1:-} == --update ]] && UPDATE=1

die() {
	echo "test262-baseline.sh: $*" >&2
	exit 1
}

[[ -d $CHECKOUT/test ]] || die "no Test262 checkout at $CHECKOUT/test
  fetch one with: scripts/test262.sh --out /tmp/report.json
  or point KILN_TEST262_CHECKOUT at an existing one"

# An emptied checkout is the failure this guard exists for: every chunk answers `pass 0 fail 0`,
# which is a clean sheet rather than an error, and the change under test takes the blame.
found="$(find "$CHECKOUT/test/built-ins" -name '*.js' -print 2>/dev/null | wc -l | tr -d ' ')"
[[ $found -gt 1000 ]] || die "Test262 checkout looks empty ($found files under built-ins)"

KILN_BIN="${KILN_TEST262_KILN:-target/release/kiln}"
[[ -x $KILN_BIN ]] || die "no kiln binary at $KILN_BIN (build with: mbx build --release -p kiln-cli)"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/kiln-test262-baseline.XXXXXXXX")"
LOG_ROOT="${KILN_TEST262_LOG_DIR:-$PWD/target/test262-baseline-logs}"
mkdir -p "$LOG_ROOT"
LOG_DIR="$(mktemp -d "$LOG_ROOT/run.XXXXXXXX")"
export KILN_TEST262_LOG_DIR="$LOG_DIR"
trap 'rm -rf "$WORK"' EXIT

python3 - "$KILN_BIN" "$CHECKOUT/test" "$BASELINE" "$WORK" "$UPDATE" "$TEST262_COMMIT" <<'PY'
import json, os, pathlib, subprocess, sys

sys.path.insert(0, str(pathlib.Path.cwd() / "scripts"))
from test262_memory import run_limited, verify_effective_cap

kiln, root, baseline_path, work, update, commit = sys.argv[1:7]
update = update == "1"

if os.path.exists(baseline_path):
    with open(baseline_path) as handle:
        baseline = json.load(handle)
else:
    baseline = {"schema": 1, "test262_commit": commit, "chunks": {}}

if baseline.get("test262_commit") != commit and not update:
    sys.exit(f"baseline was taken against {baseline.get('test262_commit')}, not {commit}")

chunks = sorted(baseline["chunks"]) if baseline["chunks"] else []
if not chunks:
    if not update:
        sys.exit("baseline holds no chunks; regenerate it with --update")
    # A first run enumerates the corpus one level below each top-level directory, which is the
    # granularity the whole file is about: small enough to name a regression, large enough that
    # the run is one process per chunk rather than one per test.
    for top in sorted(os.listdir(root)):
        top_path = os.path.join(root, top)
        if not os.path.isdir(top_path) or top == "harness":
            continue
        children = [c for c in sorted(os.listdir(top_path))
                    if os.path.isdir(os.path.join(top_path, c))]
        chunks.extend(f"{top}/{c}" for c in children) if children else chunks.append(top)

measured, regressions, gains = {}, [], []
try:
    verify_effective_cap()
except Exception as error:
    sys.exit(f"memory cap verification failed: {error}; logs retained at {os.environ['KILN_TEST262_LOG_DIR']}")
for index, chunk in enumerate(chunks, 1):
    path = os.path.join(root, chunk)
    if not os.path.isdir(path):
        sys.exit(f"baseline names {chunk}, which is not in the checkout")
    out = os.path.join(work, chunk.replace("/", "_") + ".json")
    log = os.path.join(os.environ["KILN_TEST262_LOG_DIR"], chunk.replace("/", "_") + ".log")
    with open(log, "w", encoding="utf-8") as stream:
        completed, exceeded = run_limited([kiln, "--one-shot", "test", path, "--out", out],
                                          stdout=stream, stderr=subprocess.STDOUT)
    if exceeded:
        sys.exit(f"chunk {chunk} exceeded memory cap; full log: {log}")
    if not os.path.isfile(out):
        sys.exit(f"chunk {chunk} wrote no result document (exit {completed.returncode}); full log: {log}")
    try:
        with open(out, encoding="utf-8") as handle:
            document = json.load(handle)
        totals = document["totals"]
        passed, failed = int(totals["passed"]), int(totals["failed"])
    except (OSError, ValueError, KeyError, TypeError) as error:
        sys.exit(f"chunk {chunk} has invalid result ({error}); full log: {log}")
    if passed + failed <= 0:
        sys.exit(f"chunk {chunk} has zero test results; full log: {log}")
    passed, failed = totals["passed"], totals["failed"]
    measured[chunk] = {"passed": passed, "failed": failed}
    was = baseline["chunks"].get(chunk)
    mark = ""
    if was:
        if passed < was["passed"]:
            regressions.append(f"{chunk}: {was['passed']} -> {passed} passing")
            mark = "  REGRESSED"
        elif passed > was["passed"]:
            gains.append(f"{chunk}: +{passed - was['passed']}")
            mark = f"  +{passed - was['passed']}"
    print(f"[{index}/{len(chunks)}] {chunk}  {passed}/{failed}{mark}", flush=True)

if update:
    baseline["test262_commit"] = commit
    baseline["chunks"] = measured
    with open(baseline_path, "w") as handle:
        json.dump(baseline, handle, indent=2, sort_keys=True)
        handle.write("\n")
    total = sum(c["passed"] for c in measured.values())
    print(f"\nwrote {baseline_path}: {len(measured)} chunks, {total} passing")
    sys.exit(0)

for line in gains:
    print(f"gain     {line}")
for line in regressions:
    print(f"REGRESS  {line}", file=sys.stderr)
if regressions:
    sys.exit(f"\n{len(regressions)} chunk(s) regressed; accept them with --update if intended")
total = sum(c["passed"] for c in measured.values())
print(f"\nno regressions across {len(measured)} chunks, {total} passing"
      + (f", {len(gains)} chunk(s) gained" if gains else ""))
PY
