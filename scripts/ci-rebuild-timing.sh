#!/usr/bin/env bash
#
# Record the Rust build measurements design.md §13 asks CI to keep (the four that exist in v0)
# and leave a `cargo build --timings` report behind for §12's periodic inspection.
#
# usage: ci-rebuild-timing.sh [all|clean|incremental]
#
#   clean        the from-scratch build only. Meaningless unless the target directory really
#                is empty, so the job that runs this restores no target cache and pays for a
#                full build of oxc every time.
#   incremental  the no-op and edit-driven rebuilds only, on top of whatever the target
#                directory already holds. This is the developer's actual loop, it is cheap
#                against a warm cache, and it is the part worth running on every push.
#   all          both, which is what you want when running this by hand.
#
# The split exists because §12 put Oxc behind its own crate for a reason: a clean build now
# costs minutes, and making every pull request pay that to learn a number that only trends
# usefully over weeks would make this repository's CI the slow loop it is trying to design
# away.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

MODE="${1:-all}"
case "$MODE" in
all | clean | incremental) ;;
*)
	echo "usage: ${BASH_SOURCE[0]##*/} [all|clean|incremental]" >&2
	exit 2
	;;
esac

PROBE=(python3 scripts/rebuild-probe.py run)
RESULTS="${KILN_REBUILD_RESULTS:-target/rebuild-timings.jsonl}"
rm -f "$RESULTS"

if [[ $MODE != incremental ]]; then
	# The --timings HTML lands in target/cargo-timings/ and is uploaded as a workflow artifact;
	# it is the thing §12 asks to be looked at when the critical path starts to get long.
	"${PROBE[@]}" clean --timings --json-out "$RESULTS"
fi

if [[ $MODE == clean ]]; then
	python3 scripts/rebuild-probe.py report "$RESULTS" | kiln_block
	exit 0
fi

if [[ $MODE == incremental ]]; then
	# A restored target directory belongs to some earlier commit, and the cache action strips
	# the workspace's own artifacts out of it before saving. Build once, unrecorded, so the
	# probes below start from the state a developer's tree is actually in - otherwise the
	# no-op probe would be measuring this commit's first compile.
	cargo build --workspace --locked
fi

# §13 goal: a no-op rebuild under 1 s. The half of that goal worth gating on is that cargo
# recompiles *nothing*, which is machine-independent. The wall-clock ceiling here is set five
# times the goal on purpose: it exists to catch something categorically wrong - a build script
# that reruns every time, a glob that always changes - and not to referee runner noise.
"${PROBE[@]}" noop --json-out "$RESULTS" --expect-all-fresh --max-ms 5000

# §13 "incremental build after leaf edit". kiln-cli is the leaf: nothing depends on it. The
# rule from §12 is that this must not drag the foundational crates with it.
"${PROBE[@]}" leaf-edit --json-out "$RESULTS" \
	--touch crates/kiln-cli/src/main.rs \
	--expect-rebuilt kiln-cli \
	--expect-fresh kiln-proto --expect-fresh kiln-hir --expect-fresh kiln-db \
	--expect-fresh frontend-oxc --expect-fresh kiln-daemon

# §13 "incremental build after IR edit". kiln-hir is the central compiler type crate and the
# honest stand-in for that measurement. The bytecode compiler, interpreter, native C backend and
# coordinator, query engine, and Oxc adapter sit above it; Oxc itself does not rebuild. Asserting
# that downstream crates *do* rebuild also keeps the two probes above honest: a probe that could
# never observe a rebuild would pass them vacuously. Protocol and the cache/toolchain/runtime
# implementation branches are siblings and must stay out of it.
"${PROBE[@]}" central-edit --json-out "$RESULTS" \
	--touch crates/kiln-hir/src/lib.rs \
	--expect-rebuilt kiln-hir --expect-rebuilt kiln-ir --expect-rebuilt kiln-interpreter \
	--expect-rebuilt kiln-backend-c --expect-rebuilt kiln-native \
	--expect-rebuilt kiln-db --expect-rebuilt frontend-oxc \
	--expect-rebuilt kiln-conformance \
	--expect-rebuilt kiln-daemon --expect-rebuilt kiln-cli --expect-rebuilt kiln-bench \
	--expect-fresh kiln-proto --expect-fresh kiln-host-abi --expect-fresh kiln-runtime \
	--expect-fresh kiln-artifact-cache --expect-fresh kiln-native-runtime \
	--expect-fresh kiln-native-toolchain \
	--expect-fresh kiln-test-selector --expect-fresh kiln-test262

python3 scripts/rebuild-probe.py report "$RESULTS" | kiln_block
