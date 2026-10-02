# Dictionary key hash reuse measurement

The candidate threads an already-computed canonical string-key hash into dictionary own-property lookup. Against the frozen CLI74 control at `7780d181`, the candidate at `807a1f07` changes only `heap.c`, `prelude_declarations.h`, `properties.c`, and `read.c` under `crates/native/kiln-backend-c/src/runtime/`. The exact tracked diff digest is `d7aa2341540148ccb3580131e227b3b4d09e7d8934cd852d1b4d4dfa0fdd8e7b`.

The matched microbenchmark builds both sides from the same `docs/issues/perf-key-hash/workload.mjs` source with the same `bench/harness.mjs` (SHA-256 `b041101125e8f8e789321983bd5ec1579c3a8a8d63abf584adaae43afdcaf827`). Setup and sidecar priming happen before timing. Each workload call performs 65,536 dynamic long-key property reads through a five-level dictionary/prototype chain; the expected checksum is `8421376`. Three alternating control/candidate pairs used the harness's in-process warm clock and 2,000 ms budget. All six runs passed with the expected checksum.

| Builder | Warm medians (ms/call) | Median of run medians |
| --- | --- | ---: |
| CLI74 control | 45.5, 45.5, 45.5 | 45.5 |
| Candidate | 19.5, 19.75, 19.75 | 19.75 |

That is a 2.3038x control/candidate ratio, or 56.6% lower median time on this workload. Node v26.8.1 also ran the exact workload and harness in in-process mode, verified checksum `8421376`, and reported a 3.4375 ms warm median. Node is a correctness oracle here, not the optimization control.

A bounded real HTTP check reused `bench/server/compare.py` and `drive-pipeline.mjs`: three alternating passes, one client process, eight connections, two seconds per pass, pipeline depth eight. All six runs completed with zero errors and wrong responses. Control median throughput was 40,855 requests/s and candidate was 40,472 requests/s (candidate/control 0.9906); median p50 was 1.491 vs 1.508 ms and p99 was 2.116 vs 2.137 ms. This fixture shows no material HTTP gain.

Both native sides used `--node --profile native-release`, with `NODE_ENV` unset for both. The native-release C build included `KILN_NODE_ENV_PRODUCTION=1`; this measurement does not compare production versus development settings. Candidate/control CLI, workload binary, server binary, source, harness, and per-run output hashes are recorded in [measurements.json](measurements.json). Complete raw outputs and builds remain under `target/npm-compat-evidence/key-hash-pair/` in the measurement worktree.

The speedup claim is limited to repeated dynamic long-key reads through a deep dictionary prototype chain. It does not imply broad package or application gains; the HTTP pipeline result is near parity and slightly favors the control.

Reproduction commands used the managed cache and toolchain environment. The candidate builder was produced with `source /var/tmp/kiln/kiln-npm-managed/docs/issues/npm-compat/managed-env.sh`, `unset CARGO`, `RUSTUP_TOOLCHAIN=1.97.1`, then `mbx build --offline --release -p kiln-cli`. The frozen control builder is CLI74 from the control source pin. Both native workload binaries used `env -u NODE_ENV <builder> --one-shot build docs/issues/perf-key-hash/workload.mjs --node --profile native-release --output <binary>`. The Node oracle command was `node docs/issues/perf-key-hash/workload.mjs`; the warmed pair command was `python3 docs/issues/perf-key-hash/run-pairs.py`.

The HTTP comparison used the repository driver, not a custom load client:

```sh
env -u NODE_ENV RUNTIMES=kiln WORKLOADS=pipeline PASSES=3 PROCS=1 CONNS=8 DURATION_MS=2000 PORT=18088 \
  RESULTS_DIR=<fresh-result-dir> BASELINE_SOURCE=7780d181418af3cfa52060525cf90fa62f46b7f4 \
  CANDIDATE_SOURCE=807a1f075cc1fdf30cfbb20727ef523664c95b19 \
  python3 bench/server/compare.py <candidate-server> <candidate-server> \
  --baseline <control-server> <control-server>
```
