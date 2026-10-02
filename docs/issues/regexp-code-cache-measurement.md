# Bounded RegExp code cache measurement

The process-wide cache reuses only immutable `regress::Regex` code. It keeps at most 64 entries and
8 KiB of UTF-16 pattern/flag keys; only patterns up to 128 units without `u`, `v`, or `{` are
admitted. The byte limit is a key/source-volume bound, not a compiled-heap bound. RegExp objects,
their properties, `lastIndex`, and their realm-selected prototype remain on the ordinary construction
path. Compilation runs outside the cache mutex; failures are not cached.

This follows ADR 0071's rule that shared caches remember code, never objects. It does not move
language semantics out of the interpreter (ADR 0060) or alter native provider requirements (ADR
0062). The cache only shares code across patterns that already compile under the same flags.

## Exact Test262 case

Pinned checkout: `test262` commit
`771005236e88a909635104e03ba12559688c0172`.

File: `built-ins/RegExp/character-class-escape-non-whitespace.js`, both exact variants:

- `character-class-escape-non-whitespace.js#sloppy`
- `character-class-escape-non-whitespace.js#strict`

The source constructs `/\S+/g` repeatedly in a 65,534-iteration loop. Both binaries passed both
variants. The immutable baseline binary is from the e7591c17 tree; the candidate binary includes
commit `087f526d`.

| Binary | SHA-256 | Sum of per-case elapsed times | Test runner elapsed wall |
| --- | --- | ---: | ---: |
| Baseline | `9e0e75266635a045b63668400c5ef8d16174aef9289126728ffed0bcccc7a021` | 6.807 s | 6.853 s |
| Candidate after final admission guard | `2f3c78687ab3210d6d84ce521a161eb62ee63d5aa6073c29fd6d907ed7368dee` | 1.579 s | 1.597 s |

`elapsed_nanos` in the result document is summed over the two variants; `_kiln_elapsed_millis` is
the Python runner's whole child-process wall time, and the `test ... ms` line in its log is Kiln's
reported batch time. These are distinct measurements. The candidate is 4.3x faster by summed case
time. The baseline and final candidate result documents were `baseline-raw.json` and
`candidate-final.json` under the local measurement directory; each reports 2 passed, 0 failed,
0 unsupported, 0 skipped. An earlier candidate measurement was 1.477 s summed / 1.520 s runner
wall before the last rebuild, and is excluded from the table because that binary was overwritten.

Validation: `KILN_JOBS=4 mbx test --locked -p kiln-interpreter --offline regexp` passed all six
matching tests, including bounded admission, cache limits, fresh RegExp identity/properties/cursor,
flag-key separation, and the existing lone-surrogate/UTF-16 test. The CLI release build passed.
