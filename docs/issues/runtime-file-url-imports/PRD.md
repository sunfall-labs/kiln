# Runtime file-URL imports

Status: complete

Effect Migrator constructs a file URL after resolving a migration directory at runtime. The interpreter previously rejected that URL because the file was absent from the initial module graph. GraphModuleLoader now loads computed file URLs through its existing project resolver and frontend, retaining canonical module identity and the cached namespace. Other computed specifiers retain the snapshot resolver contract.

Cache lookup precedes filesystem canonicalization so a successfully imported file retains its namespace even after the file disappears. Canonical aliases share one ModulePlan key and dependencies already in the module map retain their bindings. Actor loading uses the same graph-loading implementation.

The module-map suite passed 13 tests, including percent-encoded paths, runtime-created dependencies, shared static dependency identity, repeated namespace identity, one-time evaluation and missing-file errors.

The unchanged pinned Effect Migrator suite passed all 5 cases with zero skips, inventory errors or Node/Vitest outcome mismatches. Source commit: `be11cd16`; frozen CLI SHA256: `b5f6b9fee0c483ff872141d01973b1e8dcc5fb14199e454696a5034fff99a21e`. Full per-file report and raw case log are retained beside this note. This evidence covers interpreter loading; it is not a claim about complete native upstream coverage.
