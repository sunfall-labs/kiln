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
| Clean Rust build | `Clean build` job, on pushes to main |
| No-op compiler rebuild | `Incremental rebuilds` job |
| Incremental Rust build after leaf edit | `Incremental rebuilds` job |
| Incremental Rust build after IR edit | `Incremental rebuilds` job, editing `kiln-hir` |
| Compiler daemon startup | `kiln-bench` |
| Warm daemon request (unchanged run and body edit) | `kiln-bench`, persistent daemon/client connection |
| Warm parse/lower latency | `kiln-bench` |
| Cache hit rate and graph-cache state | `kiln-bench`, from query and daemon counters in `kiln_proto::Stats` |
| Warm interpreted execution latency | `kiln-bench`, including warm parse, IR rebuild and VM launch |
| Warm native object rebuild | `kiln-bench`, validating all generated-source/object/link cache layers |
| Link latency | `kiln-bench`, repeated links over warm native objects |
| Generated C size | `kiln-bench`, alongside object and executable bytes |
| Targeted Test262 latency | `kiln-bench`, parser/interpreter batch with explicit variants |
| Full Test262 throughput | not yet — needs a pinned external Test262 checkout and harness support |

The `kiln-bench` rows are the harness's to produce. This table says where each measurement
belongs; the `Performance` block above says what the harness actually emitted on this run.
EOF
