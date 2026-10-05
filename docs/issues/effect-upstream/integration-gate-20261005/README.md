# Integration gate failure retained

This routine gate is not a landing acceptance. The aggregate refuses mixed commit snapshots because documentation commits were made in the gate worktree during execution. Production source remained unchanged, but the gate requires identical snapshot identities. A later gate must run with HEAD and the entire tree frozen.

The crate lane also has two real failures: a manual shared-library fixture omitted the FullGui provider manifest’s required link arguments, and TypeScript import elision removed the runtime-package-alias target needed by generated code. Both must be fixed before landing. The evidence archive retains the original aggregate, step envelopes and raw logs without rewriting their verdict.

Completed lanes include native parity (645 matched, five expected refusals, zero divergences), thirdparty parity (17 matched per engine, zero divergences), and routine Test262 (97,615 passes across 102,522 selected variants, 116 deferred, zero chunk regressions). Routine coverage is not complete Test262 coverage.
