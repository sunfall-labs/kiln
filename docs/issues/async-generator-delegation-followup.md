# Async generator delegation follow-up

This is a bounded follow-up for the remaining async-generator `yield*` protocol cases. The
current production fix is frozen at source commit `31554409d2dbe8c59588405263cf91e6c7a9c93a`.

The landed fix extracts a delegated iterator-result value once at the async-generator yield
boundary, preserves getter abrupt completion through the generator's own catch/finally path, and
keeps wrapped synchronous delegates distinct from real async delegates at the compiler level.

The remaining failures are pre-existing protocol work. They cluster in:

- `yield-star-async-return.js` and `yield-star-async-throw.js`
- `yield-star-sync-return.js` and `yield-star-sync-throw.js`
- `yield-star-promise-not-unwrapped.js`
- async-generator iterator close and return cases

The next implementation should carry the compiler's existing `wrapped` binding through the
authoritative `GeneratorState.delegate` representation. Wrapped synchronous delegates await each
delegated value; manually implemented async delegates preserve a promise-valued `.value`. The
same state must govern delegated `return`, `throw`, and iterator-close completion without adding a
second delegation state machine.

Validation from the frozen tree used the pinned corpus at
`~/.cache/kiln/test262-771005236e` and the freshly built `target/debug/kiln`:

| Chunk | Selected | Pass | Fail | Unsupported | Raw log |
| --- | ---: | ---: | ---: | ---: | --- |
| `language/statements` | 18,015 | 17,635 | 367 | 13 | `/tmp/statements-after.log` |
| `language/expressions` | 21,286 | 20,578 | 392 | 316 | `/tmp/expressions-after.log` |
| `built-ins/AsyncFromSyncIteratorPrototype` | 76 | 28 | 48 | 0 | `/tmp/async-from-sync.log` |
| `language/expressions/template-literal` | 114 | 114 | 0 | 0 | `/tmp/template-literal-after.log` |
| `staging/top-level-await` | 1 | 0 | 1 | 0 | `/tmp/staging-top-level-await-after.log` |

The focused `for-await-of` run was recorded in `/tmp` during the same validation session and
reported 2,412 pass / 19 fail from 2,431 selected. The final staging chunk,
`staging/top-level-await`, was then run separately and reported 0 pass / 1 fail. The
template-literal chunk above is an additional passing spot check, not the final staging chunk.
No full Test262 rerun was performed.
