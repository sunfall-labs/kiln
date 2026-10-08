# Complete interpreter survey, 2026-10-08

All 322 original TypeScript files completed with all 11,019 unique `(file, ID, mode)` records: 10,907 passed, 67 failed and 45 upstream skips. This is a failed full survey, not acceptance. Original assertions and deadlines were preserved; the official Node/Vitest mapping has zero missing IDs or mode mismatches and 67 outcome mismatches. The report and supervisor status are retained as verified zstd archives; failures.json lists every failing record, and raw logs retain the eight affected files.

The frozen CLI SHA was 271341ecf36c91175c4151d8cdcee7b4602500875868236ca33d9a6e94b6ae76, source 5c1e7383; the run exited 1 after 2769.63 seconds with unchanged inputs, without an outer timeout or disk-guard abort. Its supervisor status has conflicting process-cleanup fields: process_group_cleanup remains pending, so this record does not claim proven process-group absence.

The later native receiver changes do not change interpreter source. This survey ran in the restricted environment; a separate Node control returned EPERM on localhost listen. HttpClient ServeErrors remain failed outcomes; the control alone does not prove their hidden errno. Network and localhost access were restored afterward. Fresh final-source full native/interpreter runs, the gate, matched Bun benchmarks and publication remain required.
