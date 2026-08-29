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

# Run cargo, through Mr. Boxington when it is installed.
#
# mbx (https://mr-boxington.jdx.dev) keeps one content-addressed build cache shared across git
# worktrees instead of a target/ per checkout, and gives concurrent cargo invocations a single
# CPU budget rather than letting each assume it owns the machine. Both matter when several
# agents or CI jobs build the same workspace at once; neither matters for a lone developer,
# which is why this is a preference and not a requirement.
#
# Deliberately a fallback rather than a dependency: mbx is young, and its own guidance is to
# rerun the plain cargo equivalent when an mbx command misbehaves. Set KILN_NO_MBX=1 to do that
# for a whole script run.
kiln_cargo() {
	if [[ -z ${KILN_NO_MBX:-} ]] && command -v mbx >/dev/null 2>&1; then
		mbx "$@"
	else
		cargo "$@"
	fi
}

# Build the timing harness and echo the path to its binary. Release, because a debug build of
# a benchmark measures the wrong thing.
kiln_build_bench() {
	kiln_cargo build --release --locked -p kiln-bench >&2
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
