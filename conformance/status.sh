#!/usr/bin/env bash
#
# Print the conformance corpus inventory and the per-test status recorded in manifest.toml.
#
# What this script can honestly say, and what it must never say:
#
# Every test in manifest.toml has status "not-yet-runnable". Kiln cannot execute JavaScript yet -
# there is no interpreter, so there is no runner, so not one of these 32 tests has ever produced
# a result. A summary line reading "32 tests, 0 failures" would be true in the arithmetic and a
# lie in the only sense anyone reads it: it says the corpus passed when the corpus did not run.
#
# This is the same failure mode scripts/lib.sh already refuses to allow. kiln_report_has_
# measurements exists because a benchmark report with no measurements in it must be called
# skipped rather than passed - "a green tick on an empty measurement is worse than no
# measurement at all". The same reasoning applies with more force here, because a conformance
# suite is the thing people most want to read a green tick off. So this script reports counts of
# tests that have not run, never counts of failures; it prints no pass rate; and it says in
# words why the numbers are what they are.
#
# It also refuses to be used as a gate. `--gate` exits non-zero for exactly as long as nothing
# can run, so that wiring this into CI early produces an honest red rather than a hollow green.
# When a runner exists, this script grows a mode that executes the corpus and compares against
# manifest.toml's expected_stdout; until then there is nothing here to compare.
#
# It sources scripts/lib.sh rather than standing alone, for two reasons. The narrower one is
# style: every script in the repository that prints for a human does so through kiln_say, and
# duplicating that here would be the beginning of a second convention. The wider one is that
# kiln_block is what mirrors this output into the GitHub job summary, and the job summary is the
# single place where a false "0 failures" would do the most damage - so this script's honesty is
# worth exactly as much as its reaching that surface. The cost is that conformance/ depends on
# scripts/, which is a real coupling, but it is a one-directional one to three tiny helpers.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR/../scripts
# shellcheck source=../scripts/lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/../scripts/lib.sh"

CONFORMANCE_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
MANIFEST="$CONFORMANCE_DIR/manifest.toml"

GATE=0
for arg in "$@"; do
	case "$arg" in
	--gate)
		GATE=1
		;;
	-h | --help)
		cat <<-'USAGE'
			usage: conformance/status.sh [--gate]

			  (no flags)  Print the corpus inventory and every test's recorded status.
			  --gate      Same output, but exit non-zero while the corpus cannot run at all.
			              Use this if you wire the corpus into CI before a runner exists, so
			              that the job is honestly red rather than vacuously green.
		USAGE
		exit 0
		;;
	*)
		echo "status.sh: unknown argument '$arg' (try --help)" >&2
		exit 2
		;;
	esac
done

if [[ ! -f $MANIFEST ]]; then
	echo "status.sh: no manifest at $MANIFEST" >&2
	exit 2
fi

# Read the manifest and emit a flat, shell-friendly digest. tomllib is used where it exists;
# where it does not (any Python before 3.11, which includes the system Python on macOS) a strict
# reader handles the subset this manifest is written in and fails loudly on anything else,
# rather than guessing. Both paths are expected to produce identical output.
kiln_conformance_digest() {
	python3 - "$1" <<-'PY'
		import re, sys

		path = sys.argv[1]

		def load_strict(text):
		    """Read only the subset manifest.toml uses; raise on anything unrecognised.

		    Every field this script needs is a single-line scalar or a single-line array of
		    strings. Triple-quoted values carry prose that no caller here reads, so they are
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
		            raise SystemExit("status.sh: cannot parse manifest line: %r" % line)
		        key, raw = match.group(1), match.group(2)
		        if raw.startswith('"""'):
		            # Skip the whole multi-line value; prose is not read by this script.
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

DIGEST="$(kiln_conformance_digest "$MANIFEST")"

corpus_line="$(printf '%s\n' "$DIGEST" | grep '^corpus	' || true)"
CORPUS_NAME="$(printf '%s\n' "$corpus_line" | cut -f2)"
CORPUS_COMMIT="$(printf '%s\n' "$corpus_line" | cut -f3)"

TESTS="$(printf '%s\n' "$DIGEST" | grep '^test	' || true)"
FIXTURES="$(printf '%s\n' "$DIGEST" | grep '^fixture	' || true)"

count() { printf '%s\n' "$1" | grep -c . || true; }

TOTAL="$(count "$TESTS")"
GUARD="$(count "$(printf '%s\n' "$TESTS" | grep '	guard	' || true)")"
EFFECT="$(count "$(printf '%s\n' "$TESTS" | grep '	effect	' || true)")"
UNRUNNABLE="$(count "$(printf '%s\n' "$TESTS" | grep '	not-yet-runnable	' || true)")"
FIXTURE_COUNT="$(count "$FIXTURES")"

kiln_say "### JavaScript conformance corpus"
kiln_say ""
kiln_say "Corpus \`$CORPUS_NAME\`, ported from source commit \`$CORPUS_COMMIT\`."
kiln_say ""
kiln_say '```'
{
	printf 'group    tests  description\n'
	printf 'guard    %5s  standalone probes, one JavaScript behaviour each\n' "$GUARD"
	printf 'effect   %5s  real Effect-TS programs, whole-library at once\n' "$EFFECT"
	printf '                %s committed input fixture(s), not counted as tests\n' "$FIXTURE_COUNT"
	printf -- '-------  -----\n'
	printf 'total    %5s\n' "$TOTAL"
} | kiln_block
kiln_say '```'
kiln_say ""

# The whole point of this script. Do not soften it, and do not add a pass rate: a pass rate over
# zero executions is not a small inaccuracy, it is the specific false statement this exists to
# prevent.
if [[ $UNRUNNABLE -eq $TOTAL ]]; then
	kiln_say "**Nothing in this corpus has run.** All $TOTAL tests are \`not-yet-runnable\`."
	kiln_say ""
	kiln_say "Kiln cannot execute JavaScript yet. There is no interpreter, so there is no"
	kiln_say "runner, so not one of these tests has ever produced a result:"
	kiln_say ""
	kiln_say '```'
	{
		printf 'executed          0\n'
		printf 'passed            0\n'
		printf 'failed            0\n'
		printf 'not yet runnable  %s\n' "$UNRUNNABLE"
	} | kiln_block
	kiln_say '```'
	kiln_say ""
	kiln_say "Read those zeroes as \"no evidence\", not as \"no problems\". Reporting this as"
	kiln_say "\"$TOTAL tests, 0 failures\" would be true arithmetic and a false claim, in the same"
	kiln_say "way an empty benchmark report is not a performance pass - see"
	kiln_say "\`kiln_report_has_measurements\` in \`scripts/lib.sh\`."
else
	RUNNABLE=$((TOTAL - UNRUNNABLE))
	kiln_say "$RUNNABLE of $TOTAL tests have a status other than \`not-yet-runnable\`, but this"
	kiln_say "script still does not execute anything - it only reports what \`manifest.toml\`"
	kiln_say "records. Any pass or fail count must come from a runner that actually ran them."
fi

kiln_say ""
kiln_say "Per test:"
kiln_say ""
kiln_say '```'
printf '%s\n' "$TESTS" | while IFS=$'\t' read -r _ _ id status features; do
	printf '%-18s  %-40s  %s\n' "$status" "$id" "$features"
done | kiln_block
kiln_say '```'
kiln_say ""
kiln_say "\`features\` follows docs/design.md §9 and is what a changed-test selector will key on."
kiln_say "Expected output for every test is recorded in \`conformance/manifest.toml\`; see"
kiln_say "\`conformance/README.md\` for how it was obtained and what still needs to exist before"
kiln_say "any of it can be checked."

if [[ $GATE -eq 1 && $UNRUNNABLE -eq $TOTAL ]]; then
	kiln_say ""
	kiln_say "Failing \`--gate\`: a suite that cannot run cannot pass."
	exit 1
fi
