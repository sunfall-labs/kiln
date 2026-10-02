# Mixed extended Test262 costs, 2026-09-26

This work covers the 48 files outside built-ins/RegExp and the six Map/Set/TypedArray limit files in the committed extended manifest. [inventory.md](inventory.md) classifies every file; [inventory.json](inventory.json) retains the exact per-file counts and before/after times. The extended manifest and conformance baseline were not changed.

## Changes

- `ec0d88e0`: the bytecode compiler indexes constants using their existing `Eq`/`Hash` identities, preserving insertion order in the serialized constant vector. The 65,536-entry casing table previously scanned the entire growing constant pool on every literal. The index removes that quadratic compiler work. Number bits, including signed zeros and NaN payloads, stay distinct. The additional table lives only while compiling; no bytecode format changes.
- `44f9b863`: proxy ownKeys duplicate/invariant membership and target key deduplication use sets while keeping the ordered result vector. CreateListFromArrayLike completes before duplicate rejection, preserving later indexed getters and their thrown values. Nonextensible extra-key checks use cardinality only after confirming all target keys are present. Ordinary ObjectData property definition/descriptor lookup still scans a vector, so this does **not** make the entire 15,000-property workload linear.

## Measurement

Baseline is the immutable executable from `80f55d78`, SHA-256 `77a6c7823e45604ee76df3073e96b4cc63da43e7f292c412bd17003000db58e2`. Final candidate is the copied binary containing both changes, SHA-256 `8091cabb43c7c7400197cd962f116a344eda5012b7cba398281e627eb4582295`. All final measurements use those copies, the pinned `771005236e88a909635104e03ba12559688c0172` corpus, `KILN_JOBS=4`, and `KILN_TEST262_MEMORY_MB=4096`, through `test262_corpus.run_result` or `scripts/test262.sh`. Host: Linux x86_64 WSL2. Other agents were active; alternating trials limit but do not eliminate shared-machine contention.

The entire 48-file batch went from 30.930s to 15.424s wall time. Summed case timings went from 74.355s to 56.132s; removing the casing straggler improves wall time more than the sum. These are one batch each, not repeated whole-suite estimates. Both selected exactly 96 variants: **54 pass, 40 fail, 2 unsupported, 0 skipped**. Comparing the complete ID-to-outcome maps found no changed, omitted or extra variant.

Three alternating before/after trials per focused file (six variants per side), using the **final** candidate. Every variant passed. Median case milliseconds:

| File | Before | After | Speedup |
|---|---:|---:|---:|
| `staging/sm/String/string-upper-lower-mapping.js` | 11576.53 | 707.46 | 16.36x |
| `staging/sm/Proxy/ownkeys-linear.js` | 2307.63 | 1389.62 | 1.66x |

[focused-results.json](focused-results.json) retains all twelve measurements per file and their variant IDs.

## Validation

- `mbx test -p kiln-ir`: 69 passed, including first-use ordering, repeated property names, signed zero and distinct NaN payloads.
- `mbx test -p kiln-interpreter proxy_own_key_invariant_source_includes_exotic_and_native_keys`: 1 passed.
- Final candidate `kiln test conformance/regress --node`: 18 passed, including the new proxy test. Its output also matches Node directly.
- Final candidate pinned `built-ins/Proxy/ownKeys` subtree: 54 variants passed.
- Final all-48 extended batch: exact before/after IDs and outcomes agree, as above.
- `cargo fmt -p kiln-ir -- --check` and `git diff --check` passed.
- Root agent owns the final repository gate; no gate was run in this worktree.

## Remaining costs and failures

The 36 step-limit failures are not all equivalent. Thirty-four variants are finite, extensive workloads: six BMP regexp parsing files, two four-byte URI sweeps, eight DST cache stress files, and one overlapping TypedArray copy followed by 524,288 assertions. Their 8-million-step budget was unchanged. They measure time to the existing limit, **not completion time**.

The other two are `staging/sm/statements/for-of-iterator-close-throw.js`. This is a tiny function, not an exhaustive sweep: returning inside try/finally triggers an iterator `return` that throws, and execution loops until the budget. This warrants a separate control-flow correctness fix; attributing it to inherent test cost would hide the problem. No change to that path is included here.

Two comment-sweep variants retain the checked dynamic-source refusal on unpaired UTF-16 surrogates. Two timezone-pair variants retain the wrong canonical enumeration (`Etc/GMT` and `Etc/UTC` both returned, despite denoting the same timezone). Two deep-WeakMap variants build their chain but stop at the missing `$262.gc` host capability; their post-GC traversal is not exercised.

Other substantial remaining work includes sparse arrays probing one million indices (a future fast path needs prototype guards), repeated iterator/callback assertions, real calendar conversion and date parsing, large regexp source parsing, typed-array comparison callbacks, and dynamic parsing of millions of empty blocks. The inventory separates these from the two eliminated quadratic scans. Source-guided diagnosis and controlled before/after probes were used; these are not claims from a statistical CPU sample.

## Reproduce

From the worktree root, `python3 docs/issues/extended-mixed/run.py /absolute/path/to/copied-kiln result-name` runs exactly `cases.json` through the capped corpus helper, retaining `result-name.json` and the complete `result-name.log`. A third argument can name an alternative case-list JSON in the same scratch directory for the focused files. Local full logs and copied binaries remain under this directory but are deliberately not committed. The committed case list is evidence input, not a change to corpus classification or coverage.
