# Method calls on declared classes (PRD steps 3-4): design

Status: design only. Written by the `methods` optimization pass, which did not implement it.

## What exists now

A method-call site (`emit_call_method`, `emit_call_prepared_method`) already has a per-site
polymorphic inline cache (`kiln_c99_site`, up to `KILN_C99_PIC_SIZE` entries, ADR 0050/0071):
receiver shape, then either an own slot or a one-step prototype path (`one_step`, the prototype's
shape, `proto_slot`). A hit is a shape compare, one shape compare on the prototype, and a slot load.
Typed entries add `kiln_c99_method_path` (receiver shape + prototype shape -> candidate code, no
closure load) and the `recv_row` columns for `this.x` reads. The `methods` pass made the *prepared*
property read (`obj.m(args)` whose callee is read before the arguments) probe that PIC inline
(`SPECULATION_INVARIANTS: method_shape_cache`) instead of calling `kiln_c99_val_prepare_method_key`.

What remains per call is not lookup but ceremony: the frame (`js_frame_enter/leave`,
`set_method_name`), `normalize_closure_receiver`, `closure_host`, and a generic body entry with boxed
arguments, unless the callee is a typed entry (`typed_direct`).

## What the checker can add

For `pts: Point[]`, `pts[i].dist()` the checker knows (a) the static receiver class `Point`, (b) the
set of classes in the program that extend it (`Point3D`), (c) which of them declare or override
`dist`, with their function indices. Export per method call site (snapshot facts, keyed by the call's
source span like the existing return/parameter facts):

    MethodSiteFact { receiver_class, candidates: [(class, function_index)], closed: bool }

`closed` is true only when every class that can reach the receiver type is in the program (no
`declare class`, no `as any`-reachable class from a dependency without source, no class expression
whose heritage is dynamic).

## Emission

1. Pre-seed, not trust. At module init (or first execution of the site) the site's PIC is taught the
   *layout* of each candidate class: `new` through the real constructor on first call already makes
   the shapes; the facts only tell the emitter which `function_index` bodies a hit can reach, so the
   call can be `switch (plain->body)` over a closed candidate list (this is what `direct` already
   does from `method_candidates`) and the typed-entry guard (`typed_entry_guard`) can be hoisted.
2. Guarded direct call. A hit is: receiver shape matches a PIC entry (existing), the entry's
   callee is a closure whose `body` is one of the candidates (existing `plain->body ==` compare). A
   miss, or `closed == false`, takes the generic path. The checker fact never replaces the shape
   compare: `as any`, `Object.setPrototypeOf`, a replaced prototype method and a Proxy all fail it.
3. Drop the frame ceremony for leaf candidates (no call, no throw sites that need a stack record):
   a candidate whose body cannot observe `js_frame` (no `new Error`, no calls that may throw with a
   captured stack) skips `js_frame_enter/leave`. This needs a "frame-free" bit in `CallPlan` computed
   from the callee graph, the same way `typed_needs.runtime` is.

## Guards and invariants

Each of the three items is a guard with its own `SPECULATION_INVARIANTS` row: the shape/path guard is
`method_shape_cache` (exists); the closed-candidate switch is a pointer-identity guard on
`closure->body` (row `method_candidate_body`, to add); the frame-free skip has no runtime guard but
a build-time proof (row `method_frame_free`). All must work under `KILN_SPECULATE=exit` and `=0`.

## Probes (`conformance/native/method-site-shape-cache.ts` covers the first group)

Polymorphic receivers; prototype method replaced mid-loop; own property shadowing; setPrototypeOf to
a sibling/null; late method on a base prototype; getters; Proxy; `as any` liars. Still to add with
the checker facts: a class added through `declare class`; a subclass created with `Reflect.construct`
and a different `newTarget`; a mixin that returns a class expression extending the receiver's class;
and a stack-trace probe (`new Error().stack` inside a candidate) proving the frame skip is only
taken when unobservable.

## Risks

- Stack traces: skipping frame records changes `Error.stack` text; node prints method names.
- Getter-defined or Proxy-trapped `dist` reaches the same site; the PIC records only data
  properties (`slot = UINT32_MAX` for accessors) so they miss, by construction.
- The module graph is open for dynamic `import()`: `closed` must be false when a program with
  dynamic import could define another subclass of the receiver class.
