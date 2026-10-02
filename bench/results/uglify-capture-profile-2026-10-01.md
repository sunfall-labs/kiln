# Uglify capture workload: native Release CPU sample

This is a diagnostic profile, not a timing result. The profiled workload was the existing
`benchmarks/uglify-capture.js` under the `npm-compat` corpus at source commit
`d6afdd0126517ce5ff59d34e1e64c02b85ebb1fc`. The production native Release executable used as
reference was `uglify-production-cli67`, SHA-256
`9238826aec4dd48964ac9e4576407f6a3213e34c52635da33e98b49275aad92e`. Its correctness oracle is
25 calls with checksum `2612876640`; the observed Release run reported 106,519 ms for the warmed
24-call batch. That timing is context for the investigation, not a measurement made by this
profile.

The reference binary is stripped. A one-off diagnostic link removed only
`-Wl,--strip-all` through a temporary `CC` wrapper. It produced
`/var/tmp/kiln/kiln-managed-npm-tmp/kiln-native-9-0-1790903655563134312/program`, SHA-256
`c090bb5fc0a3e2b091366ba850aef08f9e42071d43444a8e75e64079e77471e7`. Its `.text` section has the
same virtual address (`0xa96000`), file offset (`0xa95000`), size (`0xf31eb3`), and bytes (SHA-256
`1922d1c479fdf77b11ef83ae7a3d316f4025babd95b1c4dc764f130568e691e6`) as the stripped reference,
so sampled instruction offsets map directly to symbols in the diagnostic link.

The first sample used `ITIMER_PROF` at 2 ms and ran for 35 s of CPU before the owned process group
was terminated. It collected 17,453 instruction samples. The two largest executable FDE ranges
were `0x1507720..0x1507871` (6,253 samples, 35.83%) and `0x151f4e0..0x151f7f9` (6,040 samples,
34.61%). Symbolization identifies these as `kiln_string_equal` and
`kiln_c99_own_index_key`. The next notable ranges were `kiln_dyn_lexer_next` (595, 3.41%) and
`kiln_c99_interned_key_in` (433, 2.48%).

A second 20 s sample also recorded a bounded stack window per sample. It collected 9,973 records.
Among records sampled in `kiln_c99_own_index_key`, 2,920 showed sampled return addresses for
`kiln_c99_has_property_key`, `kiln_c99_property_read_own`, and `kiln_c99_interned_key_in` in the
stack window. These are candidate sampled callers, not a complete or validated unwind; the
profile does not establish one exact source call chain. It does establish that repeated dictionary
membership/own-property lookup is a major hotspot. `kiln_c99_own_index_key` currently scans the
ordered property vector, and `kiln_string_equal` accounts for most sampled CPU in that scan.

The sampling process intentionally stopped before the workload completed. Its empty stdout is not
an output/checksum verdict, and its elapsed time is not comparable to the prior Release benchmark.
The raw sampler, stack sampler, wrapper, retained objects, logs, and binaries were kept under
`/var/tmp/kiln/uglify-profile/` and the task-specific managed temporary directory for
reproduction.
