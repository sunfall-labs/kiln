#!/usr/bin/env bash
#
# Record the Rust build measurements design.md §13 asks CI to keep (the four that exist in v0)
# and leave a `cargo build --timings` report behind for §12's periodic inspection.
#
# The order below is load-bearing. The clean build must run first, and it must run against an
# empty target directory, or the number it reports is a fiction - which is why the workflow
# that calls this deliberately does not restore a target-directory cache.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
cd "$(kiln_repo_root)"

PROBE=(python3 scripts/rebuild-probe.py run)
RESULTS="${KILN_REBUILD_RESULTS:-target/rebuild-timings.jsonl}"
rm -f "$RESULTS"

# The --timings HTML lands in target/cargo-timings/ and is uploaded as a workflow artifact;
# it is the thing §12 asks to be looked at when the critical path starts to get long.
"${PROBE[@]}" clean --timings --json-out "$RESULTS"

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
	--expect-fresh kiln-proto --expect-fresh kiln-db --expect-fresh kiln-daemon

# §13 "incremental build after IR edit". kiln-db is v0's central type crate - it plays the role
# the IR crate will play later - so this is the worst case and everything downstream of it is
# expected to rebuild. Asserting that it *does* also keeps the two probes above honest: a probe
# that could never observe a rebuild would pass them vacuously. kiln-proto does not depend on
# kiln-db and must stay out of it.
"${PROBE[@]}" central-edit --json-out "$RESULTS" \
	--touch crates/kiln-db/src/lib.rs \
	--expect-rebuilt kiln-db --expect-rebuilt kiln-daemon --expect-rebuilt kiln-cli \
	--expect-fresh kiln-proto

python3 scripts/rebuild-probe.py report "$RESULTS" | kiln_block
