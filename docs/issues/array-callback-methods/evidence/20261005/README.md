# Final Array callback evidence

This archive contains the existing final-source validation artifacts; no test was rerun to create it. The validated candidate source is commit `3b05e086d05976d1e4ad0081ffc591176aad8bab` (parent `09cb9b75afa5b613fb105f60aaacabbc479ffdc5`), later integrated by the main tree as `f17ec9f2`. The native release CLI used for the final candidate run has SHA-256 `ecf0e5be832c12c59e7bf0aa346e1f67c76069ad1f2f5d57d1a4b152d7fb7a7a`; the frozen comparison CLI is `d0830baef65b01c9ccfeb626984e44cf863418a7008596188feb29822949ea88`. The pinned Test262 checkout is commit `771005236e88a909635104e03ba12559688c0172`.

`build-final.log` is the terminal build log for the candidate CLI. The interpreter result is retained in `interpreter-tests.log`. `native-parity.log`, `native-gc-stress.log`, and `native-realms-summary.log` retain the native summaries; `native/realms/` holds the per-process stdout, status, and build records for N=1, 2, and 8, with compiled executables excluded. `scaled-gc-stress-fixture.js` is the exact stress input: the committed native fixture was copied with only its million-hole stress scale reduced to 1,000 slots. The ordinary native fixture digest is recorded in `index.json`.

The Test262 reports retain every selected case ID and outcome, including failures, as structured JSON. Paired raw runner logs are in `test262/logs/`. Candidate and frozen-baseline totals were:

| Subtree | Candidate | Baseline |
| --- | ---: | ---: |
| Array `map` | 427/429 | 421/429 |
| Array `reduce` | 509/517 | 507/517 |
| Array `reduceRight` | 509/517 | 505/517 |
| Object `defineProperty` | 2205/2250 | 2205/2250 |
| Object `defineProperties` | 1250/1264 | 1250/1264 |

The map candidate has two failures (`create-species-abrupt.js`, sloppy and strict); the map baseline has those plus six failures listed in its report. The reduce and reduceRight candidate failures match their baselines, except each candidate removes two and four failures respectively. Both Object descriptor candidate reports have the same failing case IDs as their baselines. All per-case IDs are present in each report, and `index.json` records report totals, failed IDs, byte counts, and SHA-256 digests.

The source-validation receipt is `../../validation.md`. This archive intentionally contains reports and logs, not binaries, compilers, or a copy of the Test262 corpus.
