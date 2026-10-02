# Unicode construction optimization

Measured 2026-09-27 in owned worktree `kiln-opt-unicode`, starting at
`80f55d78644c56d8216ec75a943fe85ece809f8d`. These are comparisons against that
revision, not against the later common-bootstrap changes on main.

Baseline executable SHA-256:
`77a6c7823e45604ee76df3073e96b4cc63da43e7f292c412bd17003000db58e2`.
Optimized executable SHA-256:
`2fcce2a2cfa94d637c98afd8efa7878d0a7a416c3bea0d8e9d84ca514aa14d0f`.
Both executables were immutable copies, outside mbx's build output paths.

## Findings and changes

SIGPROF instruction-pointer sampling of the real Mongolian Unicode builder found
interpreter dispatch, numeric unary coercion, strict indexed-write validation and
per-unit string copying. The builder produces `181 2162507` UTF-16 length checksums;
regular-expression matching was not its dominant cost.

- `StringUnits::concat` copies stored Latin1/UTF16 slices in bulk, preserves lone
  surrogates, and avoids widening/narrowing Latin1 operands. Interpreter addition
  still performs its existing conversions and string-length bound first.
- Number operands use the existing infallible unary implementation directly.
  Object coercion, BigInt and Symbol paths are unchanged; bytecode steps and
  scheduling checkpoints remain unchanged.
- Strict numeric writes on ordinary arrays use the existing indexed writer when
  ordinary descriptors and the default prototype chain prove the write acceptable.
  A monotone flag in shared `Rc<Intrinsics>` invalidates the shortcut whenever any
  realm installs an indexed Array/Object prototype property. Merely materializing
  the built-in property stores does not invalidate it. Custom array prototypes,
  exotic indexed descriptors, locked lengths and closed/frozen arrays fall back.
  The existing conservative sparse flag is deliberately retained.

Discarded experiments: a sorted prototype-surface lookup and broad primitive
coercion bypass did not improve representative measurements. The first strict-write
guard checked whether intrinsic stores were empty; ordinary bootstrap materializes
those stores, so that guard was inert and was replaced with mutation metadata.

## Measurements

Every Test262 invocation used the pinned checkout `771005236e88a909635104e03ba12559688c0172`
through `scripts/test262_corpus.run_result`, with `KILN_JOBS=4` and
`KILN_TEST262_MEMORY_MB=4096`. No execution budget, baseline, classification or
case selection was weakened. All variants of the selected files ran.

| Work | Baseline | Optimized | Reduction |
| --- | ---: | ---: | ---: |
| All 448 extended RegExp files / 896 variants, wall time | 166.192 s | 125.406 s | 24.5% |
| Sum of all case execution times | 663.262 s | 496.047 s | 25.2% |

Every one of the 896 case IDs has the same passing outcome.

The representative selection spans character class escapes, ASCII, Cased,
General_Category=Letter, Script=Mongolian and Script_Extensions=Common. A preliminary
12-variant pair fell from 3.025 s to 2.393 s (20.9%); all twelve passed.
Three later alternating pairs (AB, BA, AB) measured baseline wall times
2.961 / 2.982 / 2.901 s and optimized times 2.210 / 2.234 / 2.194 s. The medians
are 2.961 s and 2.210 s: 25.4% faster. All twelve cases passed on every run.

These are local elapsed timings, subject to host load. The full selection comparison
is one run of each binary, not a statistically controlled whole-gate speedup claim.

## Validation and limits

- String/interpreter suites: 202 passed, zero failed, one ignored; the subsequent
  child-VM definition regression also passed.
- Unary operator Test262 subtrees: all 428 outcomes identical, 422 pass and six
  existing failures.
- Explicit strict numeric-write probes pass for holes, truncation/reuse, negative
  zero, non-index numeric names, frozen/closed arrays, exotic own setters, custom
  prototypes and a once-coerced object key.
- Tests cover string width combinations, split surrogate pairs, lone surrogates,
  signed zero/NaN/infinity unary results and exact instruction counts. Guard tests
  exercise materialization, cross-realm shared state, deletion, and property
  definition through a child VM's actual built-in dispatch.
- Clippy succeeds with existing warnings. Changed crates formatted; diff check passes.
- Root owns the final combined gate against current main; no separate gate was run.

An existing correctness limitation was reproduced on both immutable binaries:
a child function installing a getter-only `Array.prototype[0]` does not make a
subsequent strict array indexed assignment throw. The existing fallback ignores that
inherited descriptor; Node throws. This optimization conservatively invalidates its
guard on the mutation and leaves that existing semantic debt unchanged. Native
intrinsic prototype links currently have no mutable slot; if that representation
changes, link mutation must also invalidate this guard.

## Reproduce

```
KILN_JOBS=4 KILN_TEST262_MEMORY_MB=4096 python3 docs/issues/extended-unicode/measure.py \
  --kiln /absolute/path/to/copied-kiln \
  --checkout ~/.cache/kiln/test262-771005236e \
  --out target/unicode/reproduction --all
```

Omit `--all` for the six-file representative selection. The helper retains complete
logs/results and verifies the exact strict/sloppy case IDs, not only totals.

Local evidence: `target/unicode/all-baseline.json`, `all-baseline.log`,
`all-after/results.json`, `all-after/run.log`, `unary-baseline.json`,
`unary-after.json`, `final-unit-tests.log`, `child-guard-test.log`, `clippy.log`.
