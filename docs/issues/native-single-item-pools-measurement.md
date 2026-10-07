# Single-item native pool measurement

The serial paths avoid creating the source digest pool for zero or one file and the native emit
pool for zero or one module. They retain the existing source override and output order, and the
emitter still routes work through `run_worker`, preserving panic-to-error conversion.

## Probe

Fixture: one `main.js` copied from `conformance/native/numeric-remainder-boxing.js`, in an isolated
project containing a minimal `package.json`. Both binaries used the same project and shared
`KILN_SHARED_CACHE`; commands were `kiln --one-shot run --native main.js`. Compiler environment:
the pinned clang 18 path from `CC` and its existing `LD_LIBRARY_PATH`.

Immutable baseline e7591c17 binary SHA-256:
`9e0e75266635a045b63668400c5ef8d16174aef9289126728ffed0bcccc7a021`.

Candidate binary SHA-256:
`0319b0682b8502e20b106ea1166203367f49780718d9f5c3f49a45d398d31633`.

After one warm-up per binary, two interleaved warm samples produced:

| Binary | Warm wall samples | Peak process threads |
| --- | --- | ---: |
| Baseline | 0.460 s, 0.470 s | 15 |
| Candidate | 0.796 s, 0.458 s | 3 |

Both returned identical stdout and status 0. Timing is noisy and does not establish a latency
gain; the samples overlap. The thread high-water mark fell by 12 in this one-source, one-module
case. The warm-up is excluded: baseline took 6.311 s / 47 threads while filling the shared cache,
and candidate then took 0.603 s / 3 threads against that already-warm cache. Raw samples are in
`one-source-results.json` in the local scratch directory.

Focused validation passed:

- `KILN_JOBS=4 mbx test --locked -p kiln-cli --offline one_file_reads_and_digests_keep_the_source_snapshot`
- `KILN_JOBS=4 mbx test --locked -p kiln-native --offline empty_and_single_item_emission_stay_ordered_and_catch_panics`

The first test verifies empty inputs and snapshot substitution through both read and digest paths.
The second verifies zero/single-item ordering and the worker panic error. No threshold above one was
introduced.
