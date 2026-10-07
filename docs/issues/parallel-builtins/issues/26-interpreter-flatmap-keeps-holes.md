# 26. The interpreter's `flatMap` keeps an answer's holes

Status: resolved

Found by issue 08 (tier 3). `flatMap` flattens each callback answer that is an array through
`FlattenIntoArray`, which asks `HasProperty` for every index and skips a hole. The interpreter
appends the hole as `undefined`; node and the native backend at `native-dev` skip it.

```js
const shared = [1, , 3];
console.log(JSON.stringify([10, 20, 30, 40].flatMap((x, i) => (i & 1) ? shared : [x])));
// node, native-dev, parallelFlatMap: [10,1,3,30,1,3]
// interpreter:                       [10,1,null,3,30,1,null,3]
```

Since the interpreter is the corpus's oracle, `conformance/native/parallel-alloc-flatmap.js`
returns hole-free arrays for now; add a holey answer there once this is fixed.

## Acceptance

- The example prints node's answer in the interpreter, and `parallel-alloc-flatmap.js` gains a
  holey answer that matches at `parallelism=1,2,max`.

## Comments

2026-09-27, branch `work/miscompile-27`.

**Owner.** `flatMap` is the interpreter's Rust (`CallbackMethod::FlatMap`); `parallelFlatMap` in
`kiln-builtins/src/parallel.ts` delegates to it, so the fix is in one place (ADR 0060).

**Root cause.** Two walks implement it. The array fast path (`vm/arrays.rs`) collects answers and
`flatten`s them, which already drops `Native::Hole`. The generic walk (`vm/array_like.rs`, which
the example takes) read each answer by index, wrote an element only when `HasProperty` said it
existed -- but advanced the target index for every index, so each hole left a gap (read back as
`undefined`; `JSON.stringify` prints `null`), and a sparse answer of length 6 grew the result by 6.
FlattenIntoArray increments `targetIndex` only after it writes; the increment moved inside the
`if`.

**Tests.** `conformance/regress/r46-flatmap-skips-answer-holes.js`, recorded from node 26.8.1: the
example, leading holes, a holey receiver, an array-like receiver, a sparse answer, and `flat` for
contrast. It fails on base (`[10,1,null,3,...]`) and passes on the branch.
`conformance/native/parallel-alloc-flatmap.js` gains holey answers -- a shared holey array and
worker-built `[, x]` / `[x, , x]` answers -- matching at `parallelism=1,2,max`.

**Noticed, not fixed** (not flatMap's): `A.from([1, 2])` for `class A extends Array` answers an
`Array` in the interpreter where node answers an `A`, so every species-created result off it is an
`Array` too.

**Results.** `kiln-interpreter` 200 passed, 0 failed; `kiln test conformance/regress --node` with
the release CLI 39/39; `scripts/native.sh` 290 matched, 1 refused, 0 diverged.

2026-09-30 follow-up: the previously noted Array.from subclass bug is fixed in both engines.
Array.from constructs its receiver before reading elements, interleaves mapping with iteration,
and closes an iterator when defining a result element throws. r55 and the native-release
array-from-constructor probe cover subclasses, array-like/iterable construction order, ordinary
constructors and nonconstructors. Follow-up checks also fixed native non-extensible property
definition and subclass map species, including an inherited accessor's receiver and absent/null
species. 55/55 interpreter regressions and the focused native constructor probe pass.
