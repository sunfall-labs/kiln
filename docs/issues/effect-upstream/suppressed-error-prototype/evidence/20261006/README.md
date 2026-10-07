# SuppressedError disposal prototype fix

The TypeScript `SuppressedError` facade allocated a fresh public prototype, while interpreter
disposal constructs internal `ErrorKind::Suppressed` objects with the realm's native intrinsic
prototype. Those objects therefore failed `instanceof SuppressedError` even though their reported
name was `SuppressedError`. The facade now reuses the captured intrinsic prototype and continues
to provide its Proxy constructor and custom allocation semantics.

The focused regression `regress/r58-internal-suppressed-error-prototype` checks internally-created
errors from both `DisposableStack.dispose()` and `AsyncDisposableStack.disposeAsync()`, including
the outer and displaced error identities, `instanceof`, `constructor`, and direct prototype
identity.

The before reports are the unchanged fc5 parent candidate captured before this patch, from
`../call-entry-test262/evidence/20261006`. The after candidate is frozen at SHA-256
`8e588ebf19685f2e5afd7c931a54bac8565dffeebfe4f86a07ece178c2c49ca2` and was built from source
commit `fc5d866d82ca4965176ad75634396e5edeb27c4a` plus the working diff in this fix branch, using
Rust 1.97.1. The Test262 corpus is pinned to `771005236e88a909635104e03ba12559688c0172`; each
subtree ran through `scripts/test262.sh` with a 4096 MiB cap, one Kiln/Rayon worker, and the frozen
candidate CLI.

| Subtree | Before | After |
| --- | ---: | ---: |
| `built-ins/DisposableStack` | 184/186 | 186/186 |
| `built-ins/AsyncDisposableStack` | 206/208 | 208/208 |
| `staging/explicit-resource-management` | 82/105 | 86/105 |

`case-delta.json` lists the eight recovered variants by exact Test262 case ID. The remaining 19
staging failures are unchanged; this patch does not change any baseline. The local conformance
regress corpus passes 58/58, including the new regression.

The JSON reports and complete Test262 logs are retained beside this receipt. The local conformance
report is also retained; its summary is 58 passed, 0 failed.
