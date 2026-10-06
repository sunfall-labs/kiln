# Frozen combined replay

This is the completed interpreter replay of the unchanged `ClusterWorkflowEngine` and
`Arbitrary` files against the combined CLI frozen for the intrinsic-index investigation.

* CLI SHA-256: `5648787dead49c15a9a34839159fb683f566d41c867542e025c6bc17ec6a8269`
* Source commit/tree at build: `80f09d88926675f85f4144e172fe90a51a45e4c5` /
  `48a6d446e485053a516c34fbc423276b21fc690c`
* Harness SHA-256: `6f06bd17422ee1c290ad68cf2ef37fa8a475aa937a675c7101bbf416fc3f9f27`
* Runtime and compiler workers: 1; kiln memory cap: 4096 MiB; harness case deadlines unchanged.
* Durable job status: terminal, exit code 1, 264.09 seconds. This is expected because both files
  retain the same timeout failures as the prior baselines.

The report has all 225 registered and recorded cases. `Arbitrary` is 170 pass, 8 timeout, 0 other
failure, 0 skip; `ClusterWorkflowEngine` is 37 pass, 10 timeout, 0 other failure, 0 skip. The exact
ID/mode/outcome comparison found no missing, added, duplicate, or mode-changed IDs and no outcome
deltas. `comparison.json` records each timeout ID and independent inventory/result comparison.

`report.json`, each per-file interpreter JSON, wrapper `status.json`, `provenance.json`, and the
comparison are retained uncompressed. Raw per-file logs and the wrapper's raw stream are gzip
compressed with deterministic metadata. `sha256.json` covers every retained artifact. The live
runner output that these files were copied from is
`~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/intrinsic-index-replay-20261006/`.
