# Sparse array method `HasProperty` fast path

Measured from `e7591c17` in the owned `perf/luna-sweeps` worktree on 2026-09-27.
Both binaries are immutable copies from this worktree's release build:

- Before: `17c35c56954f29931e95580665654b48477ef83d918787eec3a685f45b2a4c96`
- After: `7a21ef76a5f2b63900928520295837a9ee076919d18584c81b60aad0d62ede27`

The fast path answers a missing array index as absent only when the index is a valid array index,
the array has no exotic indexed descriptors or custom prototype, and the shared monotone flag says
neither `Array.prototype` nor `Object.prototype` can provide an indexed property. Each iteration
checks the current array state, so callback mutations remain visible. Every other receiver or state
uses the existing generic property walk.

The pinned Test262 checkout was `771005236e88a909635104e03ba12559688c0172`. Each run used
`scripts/test262.sh` with `KILN_JOBS=4` and `KILN_TEST262_MEMORY_MB=4096`, on the three complete
method subtrees. The runner selected 1,290 variants total; all case IDs and outcomes match exactly.

| Subtree | Variants | Before case time | After case time | Pass/fail before and after |
| --- | ---: | ---: | ---: | --- |
| `forEach` | 376 | 1.028 s | 0.616 s | 368 / 8 |
| `some` | 434 | 1.253 s | 0.698 s | 428 / 6 |
| `filter` | 480 | 1.376 s | 0.766 s | 470 / 10 |
| **Total** | **1,290** | **3.656 s** | **2.080 s** | **1,266 / 24** |

The slow cases each set an element at index 999,999 on an array with six earlier elements, then
walk the array with the selected callback method. Their sloppy/strict case times fell from
0.471 s to 0.036 s for `forEach`, 0.514 s to 0.038 s for `some`, and 0.526 s to 0.035 s for
`filter`. The pre-existing failures in these subtrees remain the same.

`mbx test -p kiln-interpreter --offline` passed: 197 passed, zero failed, one ignored. The full
routine gate remains for the parent worktree.
