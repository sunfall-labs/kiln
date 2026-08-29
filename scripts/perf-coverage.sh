#!/usr/bin/env bash
#
# Print which of design.md §13's measurements CI actually records today, and which cannot
# exist yet because the thing they measure has not been built.
#
# This is here so that "CI records §13" stays a checkable claim rather than an assumption.
# When an implementation step from §14 lands, the row that step unlocks moves up, and the gap
# between the list and the workflow stays visible instead of being quietly forgotten.
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

kiln_block <<'EOF'
### design.md §13 coverage

| Measurement | Recorded by |
|---|---|
| Clean Rust build | `Build timings` job |
| No-op compiler rebuild | `Build timings` job |
| Incremental Rust build after leaf edit | `Build timings` job |
| Incremental Rust build after IR edit | `Build timings` job, using `kiln-db` as v0's central type crate |
| Compiler daemon startup | `kiln-bench` |
| Warm parse/lower latency | `kiln-bench` |
| Cache hit rate | `kiln-bench`, from the recompute counters in `kiln_proto::Stats` |
| Warm interpreted execution latency | not yet — needs §14 step 5, the IR interpreter |
| Warm native object rebuild | not yet — needs §14 step 8, the C backend |
| Link latency | not yet — needs §14 step 8 |
| Generated C size | not yet — needs §14 step 8 |
| Targeted Test262 latency | not yet — needs §14 step 7 |
| Full Test262 throughput | not yet — needs §14 step 7 |

The `kiln-bench` rows are the harness's to produce. This table says where each measurement
belongs; the `Performance` block above says what the harness actually emitted on this run.
EOF
