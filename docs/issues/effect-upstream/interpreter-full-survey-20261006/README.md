# Complete interpreter inventory survey

The frozen combined release binary completed all 322 pinned upstream Effect files and all 11,019 original cases: 10,934 passed, 40 failed, 45 upstream skips. Every case maps to the official Node/Vitest inventory, with no missing, duplicate, orphaned or mode-mismatched cases. This is complete discovery evidence, not passing acceptance: the 40 original-deadline failures remain open.

The source/binary, runner, pin, full-report digest and terminal process receipt are in `summary.json`, alongside per-file counts and every failure. Execution overlapped the routine gate and agent builds; this survey does not establish quiet performance or a regression against earlier timings. Source and five-second case budgets were unchanged. Later Date/key/compiler candidates were not part of this binary.

Failure counts: Cron: 1, Hash: 1, arbitrary/Arbitrary: 8, cluster/ClusterWorkflowEngine: 12, cluster/Sharding: 14, encoding/SchemaBinary: 3, schema/toDifferJsonPatch: 1.
