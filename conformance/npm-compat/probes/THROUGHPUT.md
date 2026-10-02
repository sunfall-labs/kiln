# Warm npm throughput probes

These probes run the same workload repeatedly inside Node's and kiln's own process. The reported
`warm_median` is milliseconds per workload call; process startup is outside that number. The comparator
requires `mode: "in-process"`, validates every checksum against Node, and retains full stdout and
stderr for each launch.

Build standalone workloads with the frozen source trees and native release profile. For bcrypt,
build the same workload from the production and typed-array-read-control trees:

```sh
kiln build conformance/npm-compat/probes/bcryptjs-throughput.mjs --node --profile native-release --output target/npm-compat-evidence/bcryptjs-production
kiln build conformance/npm-compat/probes/bcryptjs-throughput.mjs --node --profile native-release --output target/npm-compat-evidence/bcryptjs-control
python3 conformance/npm-compat/probes/compare-throughput.py bcryptjs \
  --candidate target/npm-compat-evidence/bcryptjs-production \
  --candidate-source /path/to/production-tree \
  --control target/npm-compat-evidence/bcryptjs-control \
  --control-source /path/to/typed-array-read-control-tree \
  --results conformance/npm-compat/results/bcryptjs-throughput.json
```

The control may differ in `crates/native/kiln-backend-c/src/runtime/boxed.c` only. The comparator
also pins both file contents to the reviewed one-branch delta, and requires identical tracked
inventories, workload, benchmark harness and package lock. The workload asserts the exact fixed-salt
Node hash, successful and rejected password checks, and cost factor 8. Warmup and timed launches
alternate between production and control.

The numeric loop has no reviewed runtime control. Build it once and compare its warmed timing and
checksum against Node; the report explicitly makes no optimization-gain claim:

```sh
kiln build conformance/npm-compat/probes/numeric-bitwise-throughput.mjs --node --profile native-release --output target/npm-compat-evidence/numeric-production
python3 conformance/npm-compat/probes/compare-throughput.py numeric \
  --candidate target/npm-compat-evidence/numeric-production \
  --candidate-source /path/to/production-tree \
  --results conformance/npm-compat/results/numeric-bitwise-throughput.json
```

Use `--rounds` for an even total number of timed launches (default 8) and `--warmups` for warmup
launches per binary (default 2). Evidence JSON includes source and executable SHA-256 digests,
Node's checksum, each run's in-process timing, and the raw log directory. These custom native
measurements are separate from the npm compatibility corpus runner, whose phase timings include
process startup and are not throughput measurements.
