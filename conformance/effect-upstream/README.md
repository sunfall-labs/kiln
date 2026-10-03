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
The optional interpreter lane uses Node host-clock capability. Its timer scheduler
is being brought into agreement with that capability; continuously runnable work
currently exposes incomplete timer coverage. Dependencies are
locked. The source/dependency workspace lives under `~/.cache/kiln` and is locked
for the entire run. `--workspace` selects another owned directory. `--jobs`,
`--timeout` and `--kiln-memory-mb` bound execution; each kiln process defaults to
an 8 GiB address-space cap. Bun reserves a large virtual heap, so use an external
cgroup if a physical memory cap is needed for the whole run.

Read `report.json` and per-file engine JSON/logs. An incomplete run, timeout,
missing case or unsupported runner capability fails the run. A selected subset
reports deferred files. These are correctness results, not workload timings.
The adapter's mock/reset capabilities remain under development; see
`docs/issues/effect-upstream/README.md` for the full goal and implementation plan.
