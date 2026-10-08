# Shared receiver authority validation

The native reader threads the original receiver through shared Get, computed keys and accessor dispatch. Fresh keys continue through prototype chains; builtin replacement parents are read once after the own-member authority proves absence. The partial Proxy scan and HasProperty preflight are removed. Final integration commits are db157e46 and dd59153c.

The clean release was built from 4c86f1fd, CLI SHA `1d0c39f6f15188edd9850fdacbbe5e09a58634d27920f5f7bc112b4e498a20d7`. Production crate files are identical to final integration; later 3caf0fd2 changes only the Atomics own-read fixture. Full provider manifest/archive hashes are pinned in receipts.

Official native parity matched the expanded receiver matrix, covering explicit Reflect receivers, primitive/intrinsic/custom chains, fresh-key Proxy parents, Symbol array accessors, species, and constructor statics. It also matched under GC stress. A separate Atomics own-member read control matched without invoking parent Proxy traps. Earlier assumptions that this supported read/operation must refuse were invalid; the committed control asserts its actual supported behavior.

Official native-realms matched this representative receiver probe at 1, 2 and 16 realms, both normally and with GC stress, with zero skipped/diverged probes. Raw logs and per-realm streams are retained. These are representative checks, not full native/stress corpora or Effect acceptance.

The two narrow emitted-C tests produced one size-band pass and one stale ABI-snapshot failure. Root reviewed the actual snapshot delta: receiver-aware private helpers/signatures and existing async-generator close/delegate helpers; no public builtin IDs changed. Commit aec21688 accepts that exact delta. The size guard is measured in lines, with ceiling 87,000; no band was widened. Exact current counts were not printed. The legacy 21,996-line/1,080,395-byte snapshot is not a current measurement; the correction receipt explicitly invalidates that claim. A clean rerun on snapshot-only commit 6e96ad46 passed both tests (2 passed, 0 failed, 35 filtered out); the source was unchanged and the producer confirmed process cleanup. Its receipt and raw log are retained under raw/emitted-prelude-tests-clean. This narrow check does not replace the final gate.

Full 322-file native/interpreter replays, remaining original-deadline failures, the final gate, Bun benchmarks, latest remote integration and publication remain incomplete. SHA256SUMS covers retained artifacts.
