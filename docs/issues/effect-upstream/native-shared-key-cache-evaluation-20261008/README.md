# Native shared-key cache benchmark evidence

This is a narrow microbenchmark evaluation of the native per-heap shared-key hint. It does not
establish a broad application or Effect-suite improvement, and it does not authorize adopting the
candidate. The result is mixed: the shared-key write row improved modestly while the two local-key
write controls were slower. Keep this candidate out of production pending stronger evidence.

## Frozen inputs and execution

The parent production source was `624968060241897edfc7d38b40f18aae80a3d2bc`; its frozen CLI SHA256
was `c1ee6aa03d420d95bb5c52df29bfe8a770ff4816e802442b996e13ff1da085be`. The candidate source was
`caf6e0f4312487795bf89b9bb246db747f3480e4`; its accepted retry1 CLI SHA256 was
`a843877cc4bc1d6ceffa582d59f5c178e220d8781bd8a5302deb1fbbf229de1a`. Both native builds and native timed
runs used the same Full provider manifest SHA256
`f31f0ceebf4a272400e9667d561d119e5f00d451ae3c072c4783375f6deecc9f` and archive SHA256
`fe25b4720dd0741d661b60ceceec3ba490e4941cfd21570ec23b9aecb9c14720`.

The first pair-build attempt is retained under `native-build/initial-auditor-failure/`. It built
the parent executable, then the audit incorrectly expected literal benchmark labels in generated C.
Those labels are encoded as numeric byte arrays (for example, `ku_string_12_data` contains the
shared-key row label). No candidate native executable was built
in that attempt. The corrected audit checks the embedded frozen workload source shape instead.
The serial retry under `native-build/retry1/` passed both builds and emitted-C audits. Both C
payloads match (module ID `1004854816729230588`, payload SHA256
`3f27dfd6851e74c5a250eec3ca96b2b2c461fe44fb895c6903c4af505ead7c85`, C SHA256
`b83fe837aa8339a7442efb47ba41f44eac77cb74ebbd0e63c35c60508c26d501`). The emitted `ku_fn_1`
body for `repeatedComputedWrite` calls `kiln_c99_val_write_element`; the separate pinned runtime
source audit traces this path through `kiln_c99_intern_key`. This proves caller routing, not that
every timed iteration hit the shared cache. No application-object reuse is claimed.

The timed run used Node 26.8.1 and Bun 1.4.2, with three rotated orders across production native,
candidate native, Node, and Bun. All 12 runtime/pass records exited successfully, had all four
expected rows and checksums, and recorded reaped leaders and absent process groups. Inputs and
Node/Bun executable identities were unchanged at postflight. The complete timing receipt is
`timing/timing.status.json`; individual raw stdout and stderr files are in `timing/raw/`.

## Warm medians

Values are milliseconds per workload iteration, median across three passes. Percent change is
candidate versus production native; negative means candidate was faster. Node and Bun are direct
TypeScript workload results for context. Each checksum matched the corresponding expected value
in every pass.

| Workload row | Production native | Candidate native | Change | Node | Bun |
| --- | ---: | ---: | ---: | ---: | ---: |
| Shared canonical dynamic write | 0.4453125 | 0.4140625 | −7.02% | 0.00244140625 | 0.00390625 |
| Local same-signature collision write | 0.453125 | 0.46875 | +3.45% | 0.0029296875 | 0.00396728515625 |
| Local distinct-signature write | 0.46875 | 0.4921875 | +5.00% | 0.0029296875 | 0.00390625 |
| Warmed own-slot read control | 0.091796875 | 0.0927734375 | +1.06% | 0.002227783203125 | 0.001953125 |

The read row is a regression control: the warmed own-slot PIC returns before interning, so it does
not exercise the candidate optimization. The result is limited to this synthetic, fixed workload;
the local-key slowdowns argue against a production adoption claim, and the small shared-key change
has not been established as statistically significant.

## Reproduction and file integrity

`supervisor/` contains the build, timing, and emitted-C audit scripts. `inputs/` contains the exact
workload, harness, project boundary, and Node/Bun checksum reference. `native-build/` and `cli-receipts/`
hold the complete small receipts and logs; binaries and cache payloads are deliberately excluded.
`SHA256SUMS` lists every portable file except itself. Verify with `sha256sum -c SHA256SUMS` from
this directory.
