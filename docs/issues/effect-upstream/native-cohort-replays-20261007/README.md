# Current native Effect cohort replays

These are selective results for compiler source `c2fb0b369cc5f4304eb4788441ea9a1d1b4dc409`, CLI SHA-256 `b0ac1791e1ab0a26e51711dbaa8bd833aabbb8c016d6e7921d6244faa07639fb`, published in milestone `11a45679`. They do not establish full 322-file acceptance. No assertions, test IDs, modes or original five-second deadlines changed.

| Original cohort | First replay | Warm repeat | Prior compiler control |
| --- | --- | --- | --- |
| ClusterWorkflowEngine | 41/47, 63.22s | 46/47, 34.24s | f4: 46/47, 34.19s |
| Sharding | 61/66, 59.95s | 61/66, 48.97s | prior archived f4: 61/66 |

Every replay recorded all original cases, exact Node/Vitest mapping, zero mapping errors and zero mode mismatches. Six original timeout failures remain in the warm current cohorts: the parent activity-interrupt retry-budget case, three claim-release regressions, the asynchronous-storage-read reset case, and the durable-message/entity-layer case. The full reports retain every exact ID and outcome. Runtime figures cover the whole original file, not individual-case speedups.

Unrelated compiler/test processes overlapped the first runs. The warm current runs and contemporaneous f4 Cluster control used a dedicated systemd service with CPUWeight=10000; they ran serially with no team AOT builds. External activity was recorded before and after each execution; complete host isolation is not claimed. This matched Cluster comparison does not establish a compiler-caused regression from the first 41/47 outcome.

Both staged native builds finished successfully: Cluster 1895.82s (binary SHA `d1f66124e0f6c591994943fe3dedb80675d859dc3cf9c30f3565ad17e003e48d`), Sharding 1892.02s (binary SHA `e88a3e97d90e4402516280c5a21c09d07d21d29eed79159cc190dd7cd4040e25`). A normal runner replay triggered unexpected native-object recompilation despite verified generated-C hits. That redundant build was deliberately stopped, with its interruption recorded. The verified staged binaries were then executed through an ignored wrapper using the pinned runner's unmodified execution, result parsing and exact official-oracle comparison helpers. Source pin/cleanliness, source/entry/CLI/binary/provider hashes and original markers are retained. The object-cache miss cause remains unresolved; timezone is not asserted as a cause.

The compressed archive includes successful stage receipts, the interrupted replay, all original and warm replay logs/reports, the f4 matched control, the exact wrapper, and read-only compilation/cache audits. Native binaries remain in owned scratch and are identified by hash rather than committed. Current Sharding profiling and the separate interpreter dense-graph candidate are follow-up work, not results of this receipt.
