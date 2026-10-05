# Terminal full interpreter diagnostic

The frozen 322-file survey completed with exit 1: **10,908 passed, 66 failed, 45 skipped** across all **11,019 registered cases**. Every file is complete, with exact Node/Vitest case mapping and no missing/duplicate cases, mapping errors, source-transform gaps, mode mismatches or outer process timeouts. The 66 outcome mismatches are the recorded failures; they are not accepted exceptions.

This is a diagnostic baseline, not final-source acceptance. CLI SHA-256 is `b8be79d5c64e824c3f0225024cf7d729f78d4ae68120cc45476cc57ebe616644`, built from source `4010a5c2` (code integration `f17ec9f2`). It includes the explicit real-wall-clock fix and combined Array/tier implementation, but predates final realm-guard, constructor and namespace fixes. The runner commit/digest, corrected adapter digest, pinned upstream source and resource settings remain in the original report and receipt. Original case deadlines were unchanged; one worker and an 8 GiB cap applied, with a separate 1,800-second outer process allowance.

There are **58 original five-second deadline failures** and **eight `HttpClient` `ServeError` failures**. Independent Node/Python loopback controls in the adjacent environment evidence confirm that this session refuses socket listeners with `EPERM`; these eight cases remain failures. All other recorded failures are deadline overruns. The complete failing IDs/errors/stacks are in `failures.json`, so optimization work can be checked against the actual full inventory rather than a selected subset.

`report.json.gz` preserves the complete report, inventories, outcomes, source digests and oracle mappings. `raw-logs.tar.gz` contains all 322 per-file interpreter logs plus the complete survey stream. `receipt.json` gives parsed totals and scope, and `sha256.json` records retained artifact sizes/digests. No executable or native artifact is archived here. This terminal record supersedes the earlier live survey checkpoint without changing that historical record.

Final combined-source full-suite execution, native acceptance, the required gate, and quiet Bun benchmark comparison remain outstanding.
