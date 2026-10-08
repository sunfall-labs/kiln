# Async yield-star and adjacent native validation: supplemental receipt

This note audits the retained run files; it does not modify or replace any prior receipt. The accepted release CLI was built from Rust/C source pin `3c7b4a08ef6b28475caa300c1214fb93f67e6047` (tree `a3080893506d29ff7a6edc5b2a68d1b66921529d`, Rust/C input digest `2ac8a97868a83b197620b3e025f01558549a9f6532ded4eaccc5f79da3caaa6e`). Frozen CLI SHA-256 is `1d142294e95d2be8d78ecfee00a71faf2467a2f99f0371acd08f57b98e8ca60d`. The fixture checkout is clean at `3592e98f946947842a093d6d2a647a383d9fe1a3`; its only change since the CLI code pin is the rename of `computed-string-own-write-pic.js` to `.cjs`, preserving fixture bytes (SHA-256 `ad4871f7d3410f4af9335219ab813b76220873c76cd5d8276450d43845a5c107`). This retains the intended sloppy CommonJS helper semantics; the `.js` entry is strict ESM.

Provider identity was rechecked: full ABI 82 manifest SHA-256 `f31f0ceebf4a272400e9667d561d119e5f00d451ae3c072c4783375f6deecc9f`, archive SHA-256 `fe25b4720dd0741d661b60ceceec3ba490e4941cfd21570ec23b9aecb9c14720`. Node was pinned to v26.8.1, executable SHA-256 `19235a9b678f84729464c52623f92de130a165452747c6826d3fdc13df3abcc3`. Runner phases used one worker, an 8 GiB address-space limit, a 1 GiB disk floor, 900 second outer/per-invocation limits, and process-group reap checks. These are correctness checks, not timing evidence.

## Official runner audit

- `native-001/ordinary/receipt.json`: original pre-rename `.js` attempt, exit 1; interpreter invalid for 2 probes (strict-module/sloppy helper and async iterator-result failure). This is retained as an excluded attempt.
- `native-recovery-001/ordinary/receipt.json`: official log says `native: 2 matched, 0 refused, 0 diverged` and exit 0. The receipt's `phase_passed=false` is a supervisor-parser error: that revision expected 3 matches, while the corrected `.cjs` phase intentionally contains 2 probes. Do not rewrite the original receipt.
- `native-recovery-002/stress`: `native: 2 matched, 0 refused, 0 diverged (KILN_GC_STRESS=1; 0 stress probes skipped)`; exit 0; supervisor accepted `True`; pins unchanged `True`; timeout/disk guard `False/False`; group absent/reaped `True/True`.
- `native-recovery-002/realms`: `native realms (N = 1, 2, 16): 2 matched, 0 skipped, 0 diverged`; exit 0; supervisor accepted `True`; pins unchanged `True`; timeout/disk guard `False/False`; group absent/reaped `True/True`.
- `native-recovery-002/realms-stress`: `native realms (N = 1, 2, 16; KILN_GC_STRESS=1): 2 matched, 0 skipped, 0 diverged`; exit 0; supervisor accepted `True`; pins unchanged `True`; timeout/disk guard `False/False`; group absent/reaped `True/True`.
- `native-recovery-003/realms`: `native realms (N = 1, 2, 16): 1 matched, 0 skipped, 0 diverged`; exit 0; supervisor accepted `True`; pins unchanged `True`; timeout/disk guard `False/False`; group absent/reaped `True/True`.
- `native-recovery-003/realms-stress`: `native realms (N = 1, 2, 16; KILN_GC_STRESS=1): 1 matched, 0 skipped, 0 diverged`; exit 0; supervisor accepted `True`; pins unchanged `True`; timeout/disk guard `False/False`; group absent/reaped `True/True`.
- Direct native-vs-Node `ordinary` `async-yieldstar-value` input SHA `c222d69ff3409feb84bf27aa90ba23bb09e976cceb1c5d1bf0d60fe53d648528`: Node/native exit 0/0; byte equality `True`.
- Direct native-vs-Node `ordinary` `external-async-prepared-iterator-next` input SHA `d3f5fd7290378d17a668b3d1fd2c3d0e3dd8eaa33caa2e5fc8d71ab276e1887a`: Node/native exit 0/0; byte equality `True`.
- Direct native-vs-Node `native GC stress` `async-yieldstar-value` input SHA `c222d69ff3409feb84bf27aa90ba23bb09e976cceb1c5d1bf0d60fe53d648528`: Node/native exit 0/0; byte equality `True`.
- Direct native-vs-Node `native GC stress` `external-async-prepared-iterator-next` input SHA `d3f5fd7290378d17a668b3d1fd2c3d0e3dd8eaa33caa2e5fc8d71ab276e1887a`: Node/native exit 0/0; byte equality `True`.

The first ordinary `.js` attempt at `native-001/ordinary` was invalid because the sloppy helper ran in strict ESM mode; the fixture was subsequently renamed to `.cjs` without changing its bytes. In the corrected two-probe ordinary run, the official raw log has the passing 2/2 summary and exit 0. Its supplemental supervisor expected three probes, so its `phase_passed` boolean is false; the original log and receipt remain unchanged, and the raw official summary is the correction basis.

The full `async-yieldstar-value.js` fixture failed the interpreter side of `scripts/native.sh` with `TypeError: iterator result is not an object` in `queuedOuter`. We did not classify that as native parity. Instead, Node v26.8.1 and the direct native binary were run separately, comparing exact stdout and exit status. The external `async-prepared-iterator-next.mjs` fixture was treated the same way. Both matched Node in ordinary and native-GC-stress direct runs. The full async yield-star fixture also passed native realms at counts 1, 2, and 16, both ordinary and GC-stressed. This does not claim the interpreter failure is fixed.

## Input and output identifiers

- `computed-string-own-write-pic.cjs`: `ad4871f7d3410f4af9335219ab813b76220873c76cd5d8276450d43845a5c107`
- `prepared-iterator-next.mjs`: `de8e9efb5a8436b2565adce0dfcbc8cd251fa64ba53bde6d0a7310ac135222f3`
- `async-yieldstar-value.js`: `c222d69ff3409feb84bf27aa90ba23bb09e976cceb1c5d1bf0d60fe53d648528`
- External `async-prepared-iterator-next.mjs`: `d3f5fd7290378d17a668b3d1fd2c3d0e3dd8eaa33caa2e5fc8d71ab276e1887a`

Every official phase's receipt records identical before/after CLI, provider, archive, Node, source and probe hashes. The corresponding direct-comparison receipt records each exact command, input hash, stdout hash/bytes, exit status, and comparison result. `SHA256SUMS.txt` covers the receipts, runner logs, Node control logs, release CLI, provider inputs, source probes, and local supervisors.
