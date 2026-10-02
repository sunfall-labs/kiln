# Three thirdparty bundles diverge natively after the 2026-09-05 evening landing

Status: resolved · Severity: high
site that landed with Step 2.4 (`a5aa5447`, found by bisect and by reduction alike):

- The site cached the *callee value* keyed by the receiver's shape. Two objects of one shape
  hold their own values, so `{ next: cb }` at a shared site called the first `cb` for every
  later object, and a promise queue resolved the first item's promise for every item. That is
  the silence of `rxjs` and `async-concurrency`. An own property is now remembered by slot and
  read from the receiver at the call; only a prototype's method is remembered by value.
- The cached callee was entered through `plain->body` with only its generator flag checked,
  so a body that reads `arguments` ran without the frame globals set: `mobx`'s `join("+")`
  saw no arguments and joined with a comma. Every cached or intrinsic callee now enters
  through `kiln_c99_invoke_closure`, which knows what a body needs.
- A proxy is boxed with the object prefix but is not an object; the caches read its handler
  as a shape. Every cache path now asks the collector for the kind before reading a shape.

`conformance/native/method-cache-own-property.js` holds the six-line reproduction.

Found 2026-09-05 while rebasing the direct-call work onto `origin/main` at `2c4a23ce` (the
merge of `feat/scalar-replacement`, Step 4.1, with the speculation and type-profile work
beside it). Reproduced on a clean build of `origin/main` **without** the rebased commit, so
this is in that landing, not in the direct calls. `native.sh` and the crate tests were green
on the same tree; only `thirdparty.sh` shows it, which is the corpus that landing did not run.

| bundle | node | native at `2c4a23ce` |
| --- | --- | --- |
| `ajv` | 12 lines | trap `-1 is outside the range of representable values of type 'unsigned long'` in `fn_9`; **fixed** in the direct-call landing: the inline element read cast the index to `size_t` before checking its sign |
| `async-concurrency` | 19 lines | nothing on stdout, exit 0, nothing on stderr |
| `rxjs` | 20 lines | nothing on stdout, exit 0, nothing on stderr |
| `mobx` | `array 1:a\|2:a+b\|2:z+b\|1:z` | `array 1:a\|2:a,b\|2:z,b\|1:z` |

The two silent ones are promise-driven programs whose output arrives from continuations;
a program that prints nothing and exits 0 has had its turn loop, or its first continuation,
cut off. `mobx` prints `a,b` where node prints `a+b`: an array reached `+` and answered as a
join, or a string concatenation was replaced by an array. All three lie in the emitter paths
the landing added (inline sites, integer lattice, scalar replacement); the direct-call
commits on top of it do not touch them.

Repro: `scripts/thirdparty.sh`, or for one bundle
`target/release/kiln --one-shot run --native conformance/thirdparty/rxjs.bundle.js`.

Triage: bug, native backend, the 2026-09-05 evening landing.

## Also found on the same tree

- `postcss` printed `top:0` for `top: 0`: Step 5.3's regex matchers treated `\s` inside a
  class as the whole class, so `/[^\s:]/` matched a space. Fixed: a class is the union of its
  ranges and escape sets, negated as a whole.
- Top-level `await` failed in the C compiler with an undeclared `frame`; it now refuses by
  name (`conformance/native/top-level-await-refuses.js`).

## Follow-up left open

A proxy, a generator and a collection are boxed with the object prefix and are not objects.
A cache hit reads `obj->shape` from whatever the receiver is, and a proxy is shorter than
that offset. The fill side now refuses to learn from a non-object (a heap-search kind lookup,
once per shape), so a hit's pointer compare cannot match, but the read itself is still made.
The right fix is a box bit for the non-object kinds -- pointers use 47 bits, the payload has
48 -- so a hit tests a mask and never dereferences a proxy. Cost of doing nothing: none seen;
cost of the search per hit: 3x on `class-poly`, measured.
