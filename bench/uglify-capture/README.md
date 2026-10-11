# Uglify captured-record benchmark

This compares the production borrowed-view capture path with the control's
per-invocation captured-record hydration. Start the control from the exact
production source tree and apply only the captured-record hydration change in
`crates/native/kiln-backend-c/src/runtime/dynamic.c`. The old prepared pair
based on `a04f1321` is stale and must not be used for a result.

The harness requires both source-tree arguments. By default it uses the
`capture` control and preserves the original evidence schema. Before querying Node or
launching either workload, it compares the indexed tracked-file inventories and
the actual worktree bytes and executable bits. This includes dirty tracked
overlays, so the control change can be committed or left uncommitted. The sole
permitted tracked source difference is `dynamic.c`; workload and
`package-lock.json` must also match this harness checkout's npm-compat corpus.
Untracked build output, `node_modules`, and `target` are excluded. Evidence
records both HEADs, the common-source digest, full tracked-tree digests, the
two `dynamic.c` digests, and workload/lock digests.

Create the control as a fresh worktree from the production source commit, then
port the control behavior change into its `dynamic.c`. Review that diff to
confirm it contains no other tracked changes. Install the locked npm
dependencies in both source trees and build the workload from each:

```sh
git worktree add /var/tmp/kiln-uglify-control <production-commit>

cd <production-worktree>
npm ci --prefix conformance/npm-compat
kiln build conformance/npm-compat/benchmarks/uglify-capture.js --node \
  --profile native-release --output target/uglify-capture-production

cd /var/tmp/kiln-uglify-control
# Apply the reviewed control-only dynamic.c change here.
npm ci --prefix conformance/npm-compat
kiln build conformance/npm-compat/benchmarks/uglify-capture.js --node \
  --profile native-release --output target/uglify-capture-control

cd <production-worktree>
python3 bench/uglify-capture/compare.py \
  target/uglify-capture-production \
  /var/tmp/kiln-uglify-control/target/uglify-capture-control \
  --production-source "$PWD" \
  --control-source /var/tmp/kiln-uglify-control \
  --results /tmp/uglify-capture-results.json
```

Both binaries are standalone release programs built with `--node --profile
native-release`. This bypasses the CLI, artifact-cache lookup, and development
run policy during measurement. The workload initializes Uglify before its
timers, then times the first minify call separately from a warmed 24-call batch
using the host-backed realm clock. The harness rejects unmatched source trees
before running Node or either workload. It checks call count and output
checksum against Node and between the binaries, ignoring timing fields; then
it performs two alternating warmup pairs and eight timed invocations in
balanced production/control order. Each invocation has a bounded timeout and
an isolated process group with TERM/KILL cleanup.

Evidence contains source revisions and matched-source provenance, workload,
lock and binary SHA-256 digests, Node/Python/host versions, selected `KILN_*`
settings, per-run internal and external timings, and medians. If production
changes, recreate the control worktree from that candidate and reapply the
reviewed `dynamic.c` control change before building.

## Dictionary-index control

`--control-kind dictionary-index` compares the realm-owned dictionary lookup index with a
pre-index control build of the same workload. The comparator permits only a fixed set of tracked
file differences and pins every candidate/control content digest to the reviewed implementation
(commits `8b8931ae` and `9e775ce4`); it verifies the actual tracked inventory and every other
tracked worktree byte. Evidence records the file digests, the reviewed delta digest, and both
source commit references. A different implementation needs a deliberate comparator update.

Build the candidate from the production source tree and the control from a worktree at the
pre-index content, then run:

```sh
python3 bench/uglify-capture/compare.py \
  <candidate-binary> <control-binary> \
  --production-source "$PWD" \
  --control-source <control-worktree> \
  --control-kind dictionary-index \
  --results <results.json>
```

The source paths must be the candidate and its matched pre-index control tree. The pre-index
implementation files must have the pinned `9e775ce4` content; shared later commits are fine when
their actual tracked bytes match in both trees. The comparator rejects any unreviewed file delta
or content hash.
