# Effect Hash native replay on f4

The unchanged pinned Effect `Hash` test file passed **25/25 cases**, with zero skips and zero timed-out files. All 25 cases mapped to the official Node/Vitest 5.0.3 report; inventory, mode, and outcome mismatches were all zero. This is a one-file acceptance result, not a full 322-file corpus claim.

The run used the frozen f4 source commit `f4a22dbbb9bd3df1a2322637d94629232229b5e6`, CLI SHA-256 `35fc543aea252fd0b1e8160b6f2871f5e58326f970905cb6836319e5746eddf2`, direct TypeScript entry in the pinned Effect workspace, native-release profile, provider ABI 82, one build/runtime worker, 4,096 MiB process cap, and 2,400-second outer runner allowance. The upstream case deadline remained its original 5,000 ms.

The initial AOT stage produced binary SHA-256 `acf24350ec9cdd25b34fd6139adbfdbc8c4e5cb731bc59a8caafb6af561c14b1`. It emitted 184,917,824 bytes of C and compiled a 163,561,680-byte application object (12.779 s emission, 57.369 s compile, 0.327 s link). The official replay's rebuild hit the generated-C, application, prelude, runtime, launcher, and executable caches; the replay executable matched the staged binary SHA. The replay took 8.190 s for the native process and 20.660 s total for the runner. These timings are run receipts, not a benchmark comparison.

`evidence.tar.zst` preserves the complete runner report and per-case report/log, both build logs, job specifications and terminal statuses, frozen CLI/provider manifests, and the entry digest. Executable binaries and build caches are excluded. The report records the upstream pin, harness/oracle/mapping digests, provider identity, CLI digest, and runner digest.
