# Uglify captured-record benchmark

This compares the production borrowed-view capture path with the control's
per-invocation captured-record hydration. The control source must be based on
the same production commit; `dynamic.c` hydration is the only source difference.
The currently prepared pair is based on `a04f1321`:

- production: `/var/tmp/kiln/kiln-uglify-bench`
- control: `/var/tmp/kiln/kiln-uglify-control-matched`

The workload imports and initializes Uglify before its timers, times the first
minify separately, then times 24 calls in the same process. Both binaries need
the host-backed realm clock, so build them as standalone release programs with
`--node --profile native-release`. This bypasses the CLI, artifact-cache lookup,
and development run policy during measurement. The Python harness records total
process wall time as context, but computes the performance comparison from the
in-process warmed batch only.

Install the locked npm dependencies in both source trees, build the workload
from each, then run the harness from the production tree:

```sh
npm ci --prefix conformance/npm-compat
kiln build conformance/npm-compat/benchmarks/uglify-capture.js --node \
  --profile native-release --output target/uglify-capture-production

cd /var/tmp/kiln/kiln-uglify-control-matched
npm ci --prefix conformance/npm-compat
kiln build conformance/npm-compat/benchmarks/uglify-capture.js --node \
  --profile native-release --output target/uglify-capture-control

cd /var/tmp/kiln/kiln-uglify-bench
python3 bench/uglify-capture/compare.py \
  target/uglify-capture-production \
  /var/tmp/kiln/kiln-uglify-control-matched/target/uglify-capture-control \
  --production-source "$PWD" \
  --control-source /var/tmp/kiln/kiln-uglify-control-matched \
  --results /tmp/uglify-capture-results.json
```

The harness compares call count and output checksum against Node and between
the two binaries, ignoring timing fields. It performs two alternating warmup
pairs, then eight timed invocations in balanced production/control order. Each
invocation is a standalone process with a bounded timeout and an isolated
process group; timeout cleanup sends TERM and then KILL to that group. Evidence
includes source revisions and `dynamic.c` hashes, workload and binary SHA-256,
Node/Python/host versions, selected `KILN_*` settings, per-run internal and
external timings, and medians. Recreate the control overlay from the final
production commit before running if production has advanced beyond `a04f1321`.
