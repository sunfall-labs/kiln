# Node-style self-launch and synchronous child processes

`SchemaAOTCompiler > runs generated decoders without dynamic code generation` launches
`process.execPath` with a TypeScript `--import` preload, an entry module, generated-module
arguments, and `--disallow-code-generation-from-strings`. The interpreter now follows that
ordinary launch path through the same-realm module graph and host process authority. `execFileSync`
returns captured UTF-8 output, and preserves the child exit status, signal, and output on errors.

Preload specifiers are resolved once from the launching CLI's working directory. The CLI includes
each resolved root and its transitive source closure in the synchronized source view, including
Node module-format and package-resolution metadata, then sends the same ordered canonical roots to
the daemon. The graph cache key includes those roots in order; changed preload sources invalidate
or patch the matching graph, and a different root order cannot reuse it. Runtime-created ESM graphs
also retain file-URL query and fragment identity so a later query can compile newly written source
without changing the module instance already loaded for an earlier query.

The source sync still enforces the existing project-root containment boundary. Startup preload
file URLs with query or fragment components are explicitly refused until CLI startup roots can
carry the full URL identity; dynamic runtime imports preserve those identities.

## Validation receipt

- Pinned source commit: `67ba4e46a11ccda0b6761578bfd22c04ae00167d`
- `schema/SchemaAOTCompiler` source digest:
  `6768eb858343a40003b054d6e1f6664696f17ef1e4caed9e0cf609909a766455`
- Interpreter result: 11 passed, 0 failed, 0 skipped; all 11 cases match the pinned inventory.
- Release CLI SHA-256: `9cc505b8c605b7dc4c9cb184634f0afe068183e7082bbb037805acac01713c70`
- Report: `.scratch/aot-execfilesync-workspace/results/aot-runtimegraph-final/report.json`
- Raw case log SHA-256:
  `b7fe49397ba5134be73f60584f429bd3a673d4eda3755fa3979566dbdcf47ba8`
- Affected daemon and module-eval library tests: 115 passed, 0 failed. The serial affected crate
  run across CLI, daemon, runtime, host ABI, module resolver, and module eval also passed.
- Focused tests cover startup-root key order and stale source detection, resolver cwd behavior,
  transitive preload format/resolution metadata, same-realm `NodeRun` with code-generation policy,
  cache hits and changed/reversed preloads, and dynamic file-URL query identity across a rewrite.
