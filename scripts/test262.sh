#!/usr/bin/env bash
#
# Run the pinned upstream Test262 tree without adding it to this repository.
#
# The checkout is intentionally temporary and is verified twice: once immediately after the
# fetch, and once immediately before the runner starts.  A branch name, moving remote, or dirty
# checkout must never turn a conformance number into an unrepeatable claim.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

readonly TEST262_REPOSITORY="https://github.com/tc39/test262.git"
# Verified against the upstream main branch on 2026-08-31.  This is a commit, not a branch or tag.
readonly TEST262_COMMIT="771005236e88a909635104e03ba12559688c0172"

usage() {
	cat >&2 <<-'USAGE'
		usage: scripts/test262.sh --out <path>

		Fetch the pinned upstream Test262 commit into a temporary checkout, then run the existing
		Kiln CLI test batch against <checkout>/test. The JSON report is written at <path>.

		environment:
		  KILN_TEST262_CHECKOUT  use an existing checkout, still requiring the exact commit and a
		                         clean worktree (useful for local/offline reruns)
		  KILN_TEST262_KILN      path to a prebuilt kiln binary; otherwise release-build kiln-cli
	USAGE
}

if [[ $# -ne 2 || $1 != --out || -z $2 ]]; then
	usage
	exit 2
fi

REPO_ROOT="$(kiln_repo_root)"
RESULTS="$2"
if [[ $RESULTS != /* ]]; then
	RESULTS="$REPO_ROOT/$RESULTS"
fi

TEMP_ROOT=""
OUTPUT_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/kiln-test262-output.XXXXXXXX")"
# shellcheck disable=SC2329
cleanup() {
	if [[ -n $TEMP_ROOT ]]; then
		# The directory is private to this run and contains no repository data. Keep cleanup
		# best-effort so an interrupted fetch cannot mask the useful failure above.
		find "$TEMP_ROOT" -depth -type f -delete 2>/dev/null || true
		find "$TEMP_ROOT" -depth -type d -empty -delete 2>/dev/null || true
	fi
	find "$OUTPUT_ROOT" -depth -type f -delete 2>/dev/null || true
	find "$OUTPUT_ROOT" -depth -type d -empty -delete 2>/dev/null || true
}
trap cleanup EXIT

die() {
	echo "test262.sh: $*" >&2
	exit 1
}

validate_checkout() {
	local checkout="$1" head status
	[[ -e $checkout/.git ]] || die "Test262 checkout has no .git directory: $checkout"
	head="$(git -C "$checkout" rev-parse --verify "HEAD^{commit}" 2>/dev/null)" ||
		die "cannot resolve Test262 checkout HEAD: $checkout"
	[[ $head == "$TEST262_COMMIT" ]] ||
		die "Test262 checkout is $head, expected pinned commit $TEST262_COMMIT"
	status="$(git -C "$checkout" status --porcelain --untracked-files=all)" ||
		die "cannot inspect Test262 checkout status: $checkout"
	[[ -z $status ]] || die "Test262 checkout is dirty; refusing to run a modified tree"
	[[ -f $checkout/harness/assert.js ]] || die "Test262 harness/assert.js is missing"
	[[ -d $checkout/test ]] || die "Test262 test directory is missing"
}

if [[ -n ${KILN_TEST262_CHECKOUT:-} ]]; then
	CHECKOUT="$(cd -- "$KILN_TEST262_CHECKOUT" && pwd)" ||
		die "cannot resolve KILN_TEST262_CHECKOUT"
	validate_checkout "$CHECKOUT"
else
	TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/kiln-test262.XXXXXXXX")"
	CHECKOUT="$TEMP_ROOT/test262"
	git init --quiet "$CHECKOUT"
	git -C "$CHECKOUT" remote add origin "$TEST262_REPOSITORY"
	git -C "$CHECKOUT" fetch --quiet --depth=1 origin "$TEST262_COMMIT" ||
		die "could not fetch pinned Test262 commit $TEST262_COMMIT"
	git -C "$CHECKOUT" checkout --quiet --detach "$TEST262_COMMIT" ||
		die "could not check out pinned Test262 commit $TEST262_COMMIT"
	validate_checkout "$CHECKOUT"
fi

mkdir -p "$(dirname -- "$RESULTS")"
RAW_RESULTS="$OUTPUT_ROOT/raw-results.json"

KILN_BIN="${KILN_TEST262_KILN:-$REPO_ROOT/target/release/kiln}"
if [[ ! -x $KILN_BIN ]]; then
	kiln_cargo build --release --locked -p kiln-cli >&2
fi
[[ -x $KILN_BIN ]] || die "Kiln CLI binary is not executable: $KILN_BIN"

cd "$REPO_ROOT"

# Re-check after the build and immediately before the external tree is handed to the CLI. This
# closes the race where another process edits a caller-supplied checkout after the first check.
validate_checkout "$CHECKOUT"

# The default CLI mode uses the resident daemon.  Do not add --one-shot here: a full suite must
# pay one compiler/daemon startup, then stream all selected variants through the existing batch.
set +e
python3 - "$KILN_BIN" "$CHECKOUT/test" "$RAW_RESULTS" "$RESULTS" "$TEST262_REPOSITORY" "$TEST262_COMMIT" <<-'PY'
import json
import os
import pathlib
import subprocess
import sys
import time

sys.path.insert(0, str(pathlib.Path.cwd() / "scripts"))
from test262_memory import run_limited, verify_effective_cap

	kiln, test_root, raw_path, result_path, repository, commit = sys.argv[1:]
	try:
	    verify_effective_cap()
	except Exception as error:
	    sys.stderr.write(f"test262.sh: memory cap verification failed: {error}\n")
	    raise SystemExit(1)
	started = time.monotonic()
	completed, exceeded = run_limited(
	    [kiln, "test", test_root, "--out", raw_path],
	    cwd=os.getcwd(),
	    stdout=subprocess.PIPE,
	    stderr=subprocess.STDOUT,
	    text=True,
	)
	output, _ = completed.communicate()
	elapsed = time.monotonic() - started
	print(output, end="")
	if exceeded:
	    sys.stderr.write(f"test262.sh: child exceeded memory cap; no result document (raw log: {raw_path})\n")
	if not os.path.isfile(raw_path):
	    sys.stderr.write("test262.sh: kiln wrote no result document\n")
	    raise SystemExit(completed.returncode or 1)

	try:
	    with open(raw_path, encoding="utf-8") as handle:
	        report = json.load(handle)
	except (OSError, ValueError) as error:
	    sys.stderr.write(f"test262.sh: invalid Kiln result document: {error}\n")
	    raise SystemExit(completed.returncode or 1)

	tests = report.get("tests")
	if not isinstance(tests, list) or not tests:
	    sys.stderr.write("test262.sh: Kiln result document has no test entries\n")
	    raise SystemExit(completed.returncode or 1)

outcomes = {"pass": 0, "fail": 0, "unsupported": 0, "skipped": 0}
paths = set()
for test in tests:
    if not isinstance(test, dict) or not isinstance(test.get("id"), str):
        sys.stderr.write("test262.sh: malformed test entry in Kiln result document\n")
        raise SystemExit(completed.returncode or 1)
    paths.add(test.get("path", test["id"]))
    outcome = test.get("outcome")
    if outcome not in outcomes:
        sys.stderr.write(f"test262.sh: unknown test outcome: {outcome!r}\n")
        raise SystemExit(completed.returncode or 1)
    outcomes[outcome] += 1

elapsed_millis = max(1, round(elapsed * 1000))
executed = outcomes["pass"] + outcomes["fail"]
report.update(
    {
        "corpus": "test262",
        "test262": {"repository": repository, "commit": commit},
        "discovered": len(paths),
        "executed": executed,
        "passed": outcomes["pass"],
        "failed": outcomes["fail"],
        "unsupported": outcomes["unsupported"],
        "skipped": outcomes["skipped"],
        "elapsed_millis": elapsed_millis,
        "throughput_tests_per_second": round(executed / (elapsed_millis / 1000), 3),
    }
)
temporary = result_path + f".{os.getpid()}.tmp"
with open(temporary, "w", encoding="utf-8") as handle:
    json.dump(report, handle, indent=2)
    handle.write("\n")
os.replace(temporary, result_path)
raise SystemExit(completed.returncode)
PY
status=$?
set -e
exit "$status"
