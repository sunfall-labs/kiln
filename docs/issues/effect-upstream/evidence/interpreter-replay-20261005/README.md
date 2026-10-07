# Full Effect interpreter replay — 2026-10-05

This is the complete, unmodified 322-file interpreter run against the final
integration snapshot. The raw per-file reports/logs and aggregate report are in
`results.tar.zst`; the toolchain executable is deliberately excluded and is
identified by SHA-256 below.

## Result

- Pinned source: `67ba4e46a11ccda0b6761578bfd22c04ae00167d`
- Integration source: `4b5e6c9cc68661f19599ec02ed665e9a47379270`
- CLI SHA-256: `734d4a745f4beea64662d94fb87dcb82fd897600ed584056bff1da30941ba70a`
- Runner SHA-256: `28211c0c0120db8bcfd0c9380eeb458c4ccbca0a00a0bf4cbb5026ba50af5e61`
- Files: 322 reported, 319 complete, 3 incomplete
- Cases: 10,755 pass, 140 fail, 45 skip; 21 partial cases
- Node/Vitest mapping: 11,010 mapped; 0 unmapped; 1 inventory mapping error; 210 outcome mismatches
- Runner exit: 1, because the report contains failures/incomplete files

The incomplete files were `Hash`, `ManagedRuntime`, and `cluster/Sharding`.
The complete files with failures were `Layer`, `Result`, `ai/DecisionModel`,
`cluster/ClusterWorkflowEngine`, `http/FindMyWay/params-collisions`,
`reactivity/Atom`, `schema/SchemaAOTCompiler`,
`schema/SchemaCompilerRegression`, and
`schema/representation/toJsonSchemaMultiDocument`. Atom's 125 failures all
reported Sinon fake-timers setup failing with `Cannot redefine property: Now`.

The single Node/Vitest inventory mapping error was `ManagedRuntime`, whose
candidate inventory was unavailable because that file did not complete.
Outcome comparison is consequently incomplete.

## Reproduction

The original command, runner output, per-file JSON envelopes/logs, and final
`report.json` are preserved in the archive. It used one job and one runtime
worker, an unchanged 600-second file timeout, and an 8192 MiB interpreter cap:

```sh
python3 scripts/effect-upstream.py \
  --workspace <owned-workspace> \
  --out <owned-results> \
  --engine interpreter \
  --kiln <frozen-cli> \
  --jobs 1 --runtime-jobs 1 --timeout 600 --kiln-memory-mb 8192
```

Archive SHA-256:

```text
d60b9281bc8f0f69ac3203687358869b565d2be4a9a1b81a949e8ae3ed00c4e0  results.tar.zst
```
