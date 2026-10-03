# Upstream Effect, direct TypeScript

This corpus pins the upstream files in `corpus.json`; it is separate from kiln's
small `conformance/effect` corpus. Use its dedicated runner:

```sh
mbx build --release -p kiln-cli
./scripts/effect-upstream.py --out target/effect-upstream
./scripts/effect-upstream.py --path Array --engine bun --engine interpreter --engine native --out target/effect-array
```

The default comparison is Bun versus kiln native. Both consume the original TS and
the same adapter, including upstream setup and a shared `createRequire` shim for files that use it, without bundling.
The native lane builds a `native-dev` executable and runs it with the shipped
wall-clock policy, matching Bun. Build and execution evidence are separate.
Bun preloads its own `bun:test` mock operations so namespace spies follow its
[documented ESM mocking behavior](https://bun.com/docs/test/mocks).
The shared adapter and assertion libraries remain the same.
The optional interpreter lane uses Node host-clock capability and a shared realm
origin for timer deadlines. Dependencies are
locked. The source/dependency workspace lives under `~/.cache/kiln` and is locked
for the entire run. `--workspace` selects another owned directory. `--jobs`,
`--timeout` and `--kiln-memory-mb` bound execution; each kiln process defaults to
an 8 GiB address-space cap. Bun reserves a large virtual heap, so use an external
cgroup if a physical memory cap is needed for the whole run.

Read `report.json` and per-file engine JSON/logs. An incomplete run, timeout,
missing case or unsupported runner capability fails the run. A selected subset
reports deferred files. Aggregate counts distinguish pending files, incomplete
files and their partial case records from complete files. These are correctness
results, not workload timings; build and launch time are not benchmark scores.
`runtime-targets.json` declares the pinned tests' literal `require` and
`importActual` targets. The require adapter resolves against each original test
file and keeps those targets lazy for native linking.
The adapter's mock/reset capabilities remain under development; see
`docs/issues/effect-upstream/README.md` for the full goal and implementation plan.
