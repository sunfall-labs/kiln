# Schema instantiation tier validation and original replay

Candidate source `f3b7605709d31feec6bc1396a91c6bd3c2ad6cdd`; frozen CLI SHA-256 `496b106e8a5b74491f426c32729fc8dad8cc991b963e9dc8476af077b230f120`. Candidate code is pending the repository gate and is not part of this evidence-only main commit.

The pinned original SchemaBinary test file registered and mapped all 193 cases: 192 passed, one failed, none skipped. The unchanged zero-width array allocation-slack case hit its original five-second deadline. Case mapping, modes and inventory were complete. This replay ran alongside bounded compiler lanes; quiet confirmation and a full suite remain pending. It is not a performance comparison.

The corrected instantiation metadata reaches the compiler: 27 observed instantiation compiler commands include `-O1` and `KILN_GENERIC_MIXED_TIER=1`. The observation did not capture the exact Schema instantiation command. Its fresh cached module independently records mixed-tier metadata; source digest and selected function headers are retained. Functions 165, 199 and 200 lack the cold-tier attribute, while loop driver 193 retains it. This metadata correction alone did not resolve the failing original case.

The metadata candidate at `b5388386` also completed the pinned native crate suite: 284 passed, zero failed, two ignored. That worktree differs from the combined replay source; its full raw log is retained separately. Neither result establishes complete Effect conformance.

No native executable or provider archive is included. Build, source, compiler-command, runner and exact case records are preserved below.
