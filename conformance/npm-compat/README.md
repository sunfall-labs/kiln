# npm package compatibility corpus

361 npm packages (the original 360-package set plus upm), each with a small smoke test that uses its real API.
The methodology mirrors [porffor's npm-packages
dashboard](https://porffor-packages.porch.page/): the test runs under Node for
the expected stdout, then gets compiled with kiln to a native binary, and the
binary's stdout is compared byte for byte. Same output, it passes. The smoke
tests here are kiln-authored — porffor's are not published.

This is an **informational benchmark, not a gate**. `run.sh` exits 0 even when
packages fail, and nothing here is wired into `mise run gate` or CI. The
committed baselines in `results/` are the record to compare against, not a
threshold to enforce.
For acceptance, invoke `runner.py` with `--require-pass`; it retains the complete
selected inventory before exiting nonzero on any package non-pass.

## Layout

    conformance/npm-compat/
      README.md            this file
      package.json         all 361 packages, exact-pinned versions
      packages.tsv         name / version / porffor status / test-file stem
      tests/               361 smoke tests (3-8 real API calls each, deterministic output)
      run.sh               the harness: node baseline vs kiln native, byte comparison
      results/
        baseline-2026-09-30.md    head-to-head report vs porffor's 2026-09-30 dashboard run
        baseline-2026-09-30.tsv   per-package machine-readable results
        raw/                      per-run stdout/stderr (gitignored)

## Running it

```sh
cd conformance/npm-compat
npm ci                       # once: installs the 361 pinned packages
./run.sh                     # full corpus; retains results/raw/<run-id>/ evidence
./run.sh ms lodash           # subset by package name
TIMEOUT_S=300 ./run.sh       # per-package timeout (default 600s)
```

Each invocation retains a unique directory under `results/raw/` with full Node,
compiler, and native stdout/stderr, a TSV, and `results.json`. Unknown filters,
duplicate inventory names, and missing tests are infrastructure errors. Package
failures remain informational. Compilation and binary execution have separate
600-second limits and separate durations; `compile-timeout` and `runtime-timeout`
identify the failed phase. These wall times include process startup and are
**not** warmed workload throughput measurements. The JSON records selected and
deferred coverage, inventory and CLI digests, exact commands, exit codes and timing.
Builds use `--node` to install the Node host and its shared Buffer/web bootstrap,
matching the reference runtime. The JSON records this host mode. The original
baseline predates this runner correction and is historical evidence.
Runs default to four native workers (`KILN_JOBS` overrides this) and use
`TZ=UTC`. Sampled simplex noise uses explicit seeds so its numeric
output compares identical permutations across engines.

`run.sh` builds `target/release/kiln` first if it is missing. Two environment
quirks it handles: the native backend shells out to `cargo`/`rustc` (via the
`CARGO`/`RUSTC` env vars, not just `PATH`), and it stages a ~2GB
number-provider build under `$TMPDIR`, so `TMPDIR` defaults to `~/.cache/kiln/npm-tmp`
instead of a small tmpfs `/tmp`. This short path also keeps daemon sockets
below Unix path limits in deeply nested worktrees; override with `KILN_NPM_TMPDIR`.

`mise run npm-compat` does the same from the repo root.

For explicit profile acceptance, use the Python runner directly with a frozen CLI:

```sh
env -u NODE_ENV python3 conformance/npm-compat/runner.py \
  --kiln /absolute/path/to/kiln --raw /absolute/path/to/fresh-results \
  --profile native-dev --require-pass
```

Omit package filters for full coverage, and require 361 selected, completed and
discovered cases with zero deferred in `results.json`. The Python runner pins
`TZ=UTC` for Node, compilation and native execution even when invoked directly;
evidence records the effective timezone and policy. `native-release` aligns
Node, compilation and binary execution with `NODE_ENV=production`. The original
`warning` smoke assumes development logging and fails its Node oracle in production;
it remains visible as `node-baseline-failed` and makes `--require-pass` fail. A
Release inventory with that result is not complete passing acceptance.

## Baseline (2026-09-30)

kiln release build, Node v24 baseline:

| Status | kiln | porffor (same 360) |
|---|---|---:|
| Pass | **240 (66.7%)** | 123 (34.2%) |
| Wrong output | 11 (3.1%) | 44 (12.2%) |
| Runtime error | 94 (26.1%) | 100 (27.8%) |
| Compile error | 14 (3.9%) | 5 (1.4%) |
| Timeout | 1 (0.3%) | 0 (0.0%) |

Corpus caveat: the package list was extracted from porffor's dashboard, which
headlines 363 packages (125 pass) but rendered 360 rows (123 pass) — the
extraction is short 2 pass rows and 1 missing-API row. All numbers above are
over the shared 360.

The full head-to-head — including the 136 packages kiln passes that porffor
fails, the 19 where it is the other way round, and per-package failure
reasons — is in `results/baseline-2026-09-30.md`.
