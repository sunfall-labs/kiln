# Shared runtime comparison suite

This is a small, explicit selection of 24 compatibility probes and five workloads, not a full
language conformance suite. It tests the same answers under Node, Bun, kiln native, Perry native,
Porffor native and Static Hermes native. Source checking is measured separately on the website.

Compatibility groups: 14 language probes, six host probes, and four bundled npm API smoke tests.
Every selected probe appears in the result, including compile errors, execution errors, timeouts
and wrong output. Node supplies expected output. Matching decoded text and the original stdout
SHA-256 are both required. npm probes use the same esbuild neutral bundle for every runtime;
they measure the selected package APIs, not each runtime's package resolver or upstream test suite.
Dependencies are exact-pinned in package.json and package-lock.json.

Workloads:

- startup: print one string; includes process spawn and boot, not compilation.
- json-transform: a 5,000-row aggregate and sort program adapted from kiln v0.1.0; uses a 32-bit seeded generator, explicit counters for all cities, and non-null assertions for known indices.
- template-render: the kiln v0.1.0 2,000-row HTML escaping and digest program.
- warm-numeric: 100,000 iterations of a 32-bit multiply/add recurrence per call.
- warm-json: parse and stringify a 250-row JSON table, with numeric aggregation.

CLI timings are external whole-process wall time. The two warmed workloads perform 200 warmup
calls, then calibrate multiples of 16 calls to at least 100 ms using the process's real Date clock.
Every timed return contributes to a checksum checked against Node's 16 reference answers.
A zero/short clock, malformed result, failed process or divergence has no timing score.
Seven process runs per workload are interleaved in a reproducible shuffled order; the table shows
median and min/max, not a claim of statistical significance. Compilation finishes before timing.
The optional CPU affinity applies equally to every runtime. These are single-core workloads.

Use a checkout you own, quiet the host, and supply frozen toolchain binaries:

```sh
npm ci --prefix benchmarks --ignore-scripts
export KILN_BIN=/path/to/kiln-0.1.0/bin/kiln
export PERRY_BIN=/path/to/perry
export PORF_BIN=/path/to/porf
export SHERMES_BIN=/path/to/hermes-build/bin/shermes
export SHERMES_SOURCE_REV=<full-hermes-commit>
export ESBUILD_BIN=/path/to/esbuild
python3 scripts/compare-runtimes.py \
  --out data/runtime-comparison-2026-10-12.json \
  --work .scratch/comparison-run --passes 7 --cpu 15
```

Choose an allowed CPU on your machine, or omit --cpu and record the different conditions.
`--only <probe-or-workload>` diagnoses one case; such a report explicitly records deferred coverage.
`--runtimes node,bun,kiln,perry,porffor` runs an explicit selection; Node must be present as oracle.
The complete report records toolchain binary hashes, versions, source hashes, commands, raw output,
all timing samples, selected/deferred counts and host load. Logs and executables stay in --work.
It exits successfully even when competitors fail a probe; the statuses, not the exit code, are
the compatibility evidence. Oracle/preparation infrastructure failures abort the run.

Profiles: kiln native-release, Perry default native with its prebuilt full stdlib (no V8 fallback
flag), Porffor native -O3, shermes -O. On Linux, the tested shermes links its native executable to
shared Hermes runtime libraries; -static-link is rejected by that compiler on this platform.
The benchmark executes compiled native code, not HBC through hvm. Artifact bytes alone therefore
do not establish a comparable deployment-size ranking. Perry source-optimized runtime rebuilding
is also a different profile from the prebuilt release tested here.

The recorded toolchains are kiln v0.1.0, Perry v0.5.1520, Porffor alpha-17 and Static Hermes
commit 820aa0d4b5fe5d3ed7900e1274211ef561081b3f. Release URLs and archive hashes for Perry and
Porffor are in the result. The kiln binary is the published Linux x86_64 release archive, with
its companion native provider libraries. Node and Bun versions and binary hashes are recorded too.

Build the exact Static Hermes revision in a private checkout (CMake 4.4.3 in the recorded run):

```sh
git clone --branch static_h https://github.com/facebook/hermes.git hermes
git -C hermes checkout 820aa0d4b5fe5d3ed7900e1274211ef561081b3f
cmake -S hermes -B hermes-build -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_POLICY_VERSION_MINIMUM=3.5 -DHERMES_ENABLE_TEST_SUITE=OFF
cmake --build hermes-build --target shermes-dep -j 2
```

The fixtures are in an npm project, so kiln automatically provisions its Node layer even for
portable workloads. That is part of the tested application profile. A standalone source directory
without package.json, tsconfig or node_modules is a different startup/build configuration.
