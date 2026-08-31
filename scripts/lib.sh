# shellcheck shell=bash
#
# Shared helpers for the CI scripts. Source this; do not execute it.
#
# Every script in scripts/ is meant to be runnable by a developer on a laptop as well as by a
# GitHub runner, so nothing here may depend on the Actions environment being present.

# The scripts always live in <root>/scripts, so derive the repository root from the file's own
# location rather than trusting the caller's working directory.
kiln_repo_root() {
	cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd
}

# Copy stdin to the GitHub job summary as well as to the log, so a local run and a CI run show
# the same text and the summary never drifts from what the log says happened.
kiln_block() {
	if [[ -n ${GITHUB_STEP_SUMMARY:-} ]]; then
		tee -a "$GITHUB_STEP_SUMMARY"
	else
		cat
	fi
}

kiln_say() {
	printf '%s\n' "$*" | kiln_block
}

# Share compilation cache and CPU budgeting when the developer tool is installed, while keeping
# every script runnable on a stock GitHub runner. Keep direct `cargo` only in probes that measure
# Cargo itself (`ci-rebuild-timing.sh` and `rebuild-probe.py`).
kiln_cargo() {
	if command -v mbx >/dev/null 2>&1; then
		mbx "$@"
	else
		cargo "$@"
	fi
}

# Build the timing harness and the sibling CLI it launches for the real daemon-startup scenario,
# then echo the harness path. Release, because a debug build of a benchmark measures the wrong
# thing.
kiln_build_bench() {
	kiln_cargo build --release --locked -p kiln-bench -p kiln-cli >&2
	printf '%s\n' "$(kiln_repo_root)/target/release/kiln-bench"
}

# True when the harness's JSON report actually contains measurements. `kiln-bench` grows its
# scenarios as the compiler does; a report with none must be called skipped rather than passed,
# because a green tick on an empty measurement is worse than no measurement at all.
kiln_report_has_measurements() {
	python3 - "$1" <<-'PY'
		import json, sys
		try:
		    with open(sys.argv[1], encoding="utf-8") as handle:
		        report = json.load(handle)
		except (OSError, ValueError):
		    sys.exit(1)
		sys.exit(0 if report.get("measurements") else 1)
	PY
}

# ---------------------------------------------------------------------------------------------
# Conformance corpus.
#
# Both readers below live here rather than in their one obvious caller, because
# conformance/status.sh and scripts/ci-conformance.sh have to agree exactly on what the corpus
# contains and on what a recorded result says. A second parser that drifted from the first would
# let the status page and the gate report different numbers for the same two files, which is the
# specific kind of failure the gate exists to prevent.
# ---------------------------------------------------------------------------------------------

# Read conformance/manifest.toml and emit a flat, shell-friendly digest:
#
#     corpus   <name>    <source commit>
#     fixture  <id>      <path>
#     test     <group>   <id>   <status>   <feature,feature,...>
#
# tomllib is used where it exists; where it does not (any Python before 3.11, which includes the
# system Python on macOS) a strict reader handles the subset this manifest is written in and
# fails loudly on anything else, rather than guessing. Both paths produce identical output.
kiln_conformance_manifest_digest() {
	python3 - "$1" <<-'PY'
		import re, sys

		path = sys.argv[1]

		def load_strict(text):
		    """Read only the subset manifest.toml uses; raise on anything unrecognised.

		    Every field a caller needs is a single-line scalar or a single-line array of
		    strings. Triple-quoted values carry prose that no caller reads, so they are
		    skipped wholesale rather than parsed.
		    """
		    doc, tests, table = {}, [], None
		    lines, i = text.splitlines(), 0
		    while i < len(lines):
		        line = lines[i].strip()
		        i += 1
		        if not line or line.startswith("#"):
		            continue
		        if line.startswith("[["):
		            name = line[2:-2].strip()
		            table = {} if name in ("test", "fixture") else None
		            if table is not None:
		                (tests if name == "test" else doc.setdefault("_fixtures", [])).append(table)
		            continue
		        if line.startswith("["):
		            table = doc.setdefault(line[1:-1].strip(), {})
		            continue
		        match = re.match(r'^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$', line)
		        if not match:
		            raise SystemExit("manifest.toml: cannot parse line: %r" % line)
		        key, raw = match.group(1), match.group(2)
		        if raw.startswith('"""'):
		            # Skip the whole multi-line value; prose is not read by any caller.
		            if not (len(raw) > 5 and raw.endswith('"""')):
		                while i < len(lines) and not lines[i].rstrip().endswith('"""'):
		                    i += 1
		                i += 1
		            continue
		        if table is None:
		            continue
		        if raw.startswith('"'):
		            table[key] = raw[1:raw.rindex('"')]
		        elif raw.startswith('['):
		            table[key] = re.findall(r'"([^"]*)"', raw)
		        elif re.match(r'^-?\d+$', raw):
		            table[key] = int(raw)
		    doc["test"] = tests
		    doc.setdefault("fixture", doc.pop("_fixtures", []))
		    return doc

		try:
		    import tomllib
		    with open(path, "rb") as handle:
		        doc = tomllib.load(handle)
		except ImportError:
		    with open(path, encoding="utf-8") as handle:
		        doc = load_strict(handle.read())

		corpus = doc.get("corpus", {})
		print("corpus\t%s\t%s" % (corpus.get("name", "?"), corpus.get("source_commit", "?")))
		for fixture in doc.get("fixture", []):
		    print("fixture\t%s\t%s" % (fixture.get("id", "?"), fixture.get("path", "?")))
		for test in doc.get("test", []):
		    print("test\t%s\t%s\t%s\t%s" % (
		        test.get("group", "?"),
		        test.get("id", "?"),
		        test.get("status", "?"),
		        ",".join(test.get("features", [])),
		    ))
	PY
}

# Read a recorded conformance results document - schema 1, specified in full at the top of
# scripts/ci-conformance.sh - and emit:
#
#     meta     <recorded>   <runner>   <test count>
#     outcome  <id>         <outcome>
#
# Everything this rejects, it rejects on stderr with a non-zero status. A results document that
# cannot be understood must never reach a caller as a corpus that merely happened to score zero:
# that is the same lie kiln_report_has_measurements refuses to let an empty benchmark report
# tell, and a conformance suite is the thing people most want to read a green tick off.
kiln_conformance_results_digest() {
	python3 - "$1" <<-'PY'
		import json, sys

		# The vocabulary is kiln_proto::TestOutcome plus the manifest's own "not-yet-runnable",
		# which is what a test that no runner has ever reached is recorded as.
		OUTCOMES = ("pass", "fail", "unsupported", "skipped", "not-yet-runnable")

		path = sys.argv[1]

		def die(problem):
		    sys.stderr.write("conformance results %s %s\n" % (path, problem))
		    raise SystemExit(1)

		try:
		    with open(path, encoding="utf-8") as handle:
		        doc = json.load(handle)
		except OSError as error:
		    die("cannot be read: %s" % error.strerror)
		except ValueError as error:
		    die("is not valid JSON: %s" % error)

		if not isinstance(doc, dict):
		    die("is not a JSON object")
		if doc.get("schema") != 1:
		    die("declares schema %r, but this repository reads schema 1" % doc.get("schema"))

		tests = doc.get("tests")
		if not isinstance(tests, list) or not tests:
		    die("has no non-empty `tests` array")

		seen, counts = set(), dict.fromkeys(OUTCOMES, 0)
		for entry in tests:
		    if not isinstance(entry, dict):
		        die("has a `tests` entry that is not an object: %r" % (entry,))
		    test_id, outcome = entry.get("id"), entry.get("outcome")
		    if not isinstance(test_id, str) or not test_id:
		        die("has a `tests` entry with no `id`: %r" % (entry,))
		    if outcome not in OUTCOMES:
		        die("records outcome %r for %s; expected one of %s"
		            % (outcome, test_id, ", ".join(OUTCOMES)))
		    if test_id in seen:
		        die("records %s twice" % test_id)
		    seen.add(test_id)
		    counts[outcome] += 1

		# `totals` is optional, but a total that disagrees with the per-test detail it claims to
		# summarise is precisely the defect this gate exists to catch, so the disagreement is an
		# error rather than a preference for one side of it.
		totals = doc.get("totals")
		if totals is not None:
		    if not isinstance(totals, dict):
		        die("has a `totals` that is not an object")
		    counted = {
		        "total": len(tests),
		        "passed": counts["pass"],
		        "failed": counts["fail"],
		        "unsupported": counts["unsupported"],
		        "skipped": counts["skipped"],
		        "not_yet_runnable": counts["not-yet-runnable"],
		    }
		    for key, value in sorted(counted.items()):
		        if key in totals and totals[key] != value:
		            die("says totals.%s is %r, but its own per-test detail says %d"
		                % (key, totals[key], value))

		print("meta\t%s\t%s\t%d"
		      % (doc.get("recorded") or "?", doc.get("runner") or "-", len(tests)))
		for entry in sorted(tests, key=lambda entry: entry["id"]):
		    print("outcome\t%s\t%s" % (entry["id"], entry["outcome"]))
	PY
}
