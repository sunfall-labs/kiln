# Published checkpoint gate

`mise run gate` on clean source `f0eefa7eedede3dfdc40a6e71815e28226b318cb` finished with a failed aggregate (10 steps; 9,767 seconds). The source was pushed to main at the user’s explicit request as an incomplete checkpoint, and remote HEAD was verified. The exact gate source snapshot and CLI digest are recorded in every envelope.

Native parity passed: 676 matched, 5 declared refusals, 0 divergences. Routine Test262 met every chunk baseline with 97,729 passes among 102,522 selected variants; 116 extended variants were deferred. This is routine coverage. Workspace tests recorded 2,789 passes, 3 failures and 12 ignored cases across 110 suites. Node corpus passed 40/40; server passed 7 with 2 declared skips.

Five steps failed: benchmark binaries waited past their 7,200-second release-marker deadline; thirdparty async-concurrency omitted awaited iterator cleanup; regress worker-transfer output was nondeterministic; facts snapshots retained removed iterator argument-array allocations; and crate tests failed the same facts snapshot, a stale absent-answer function inventory row, and a scheduler test that assumed a callback’s notification meant its completion status was already published. Subsequent focused repairs and checks are separate evidence and do not turn this aggregate into a passing gate.

Compressed aggregate, envelopes and full relevant raw logs are preserved with verified compressed/uncompressed hashes. Complete Test262 chunk result documents remain in the original gate evidence directory. Final-source full Effect replays, successful gate and Bun comparisons remain required.
