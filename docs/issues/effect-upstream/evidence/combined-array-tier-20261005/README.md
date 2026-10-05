# Combined Array and bounded native tier check

Source: `ea8cc4c1357d8ae43566c1f95e8622801366cbff`; implementation integration: `f17ec9f2`.

Pinned Rust 1.97.1, mbx, offline, one Cargo worker and one test thread.
Command: `mbx test -p kiln-interpreter --offline -- --test-threads=1`.

Result: 264 passed, 0 failed, 1 ignored; doc tests 0.
Full log SHA-256: `9f87990a944ba3f6fa71413006eba39ced0500f65e139720c5b1c84acdcd50b5`.

This is a narrow interpreter crate check, not a complete gate or full Effect acceptance.
The constructor follow-up and native Sharding performance evaluation remain pending.
Latest local milestones have not been pushed: main Git metadata is read-only in the current session and GitHub DNS resolution fails. Last verified own public push: `bc82ca8ffc0fed926d8590a1272fc42fafbabb71`.

Runner unit check: `python3 scripts/effect_upstream_test.py`, 28 tests passed on source `08fb033a`. Log SHA-256: `4ce4d15bb1cbb94ceb6d55854ee518dcbd6cd12ac1de64167fdb1e88b20997c0`.

Follow-up review found that the interpreter indexed-prototype fast-path guard was shared across realms but recomputed using only the mutated realm. A realm-local correction is in progress; the earlier crate pass does not prove that cross-realm case.
