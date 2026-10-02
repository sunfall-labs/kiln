// kiln-builtins: the parallel collection family (ADR 0058, ADR 0068, ADR 0069).
//
// Shared by both interpreter and native execution. These are kiln's own members, declared in
// `kiln_builtin_surface::KILN_EXTENSIONS` and installed only when the realm installs extensions
// (`extensions.ts`), non-enumerable like every built-in method.
//
// This file is the one definition of every member of the family, on both owners, and it is the
// sequential one. Neither engine answers a member natively: `Array.prototype.parallelMap` used to
// be a description row that the interpreter (`CallbackMethod::ParallelMap`) and the C runtime
// (`KILN_C99_METHOD_ARRAY_PARALLEL_MAP`) each answered as `map` under another name, and
// `%TypedArray%.prototype.parallelMap` lived in `typedarray.ts` -- three definitions of one
// method, one of which read `this.map` at call time.
//
// The interpreter always runs these definitions. The native backend runs them too, except at a
// call site whose callback it proves pure (`crates/native/kiln-backend-c/src/parallel.rs`): there
// it runs the same computation on the pool, and otherwise lowers the site as the sequential method
// below would run (`emit_graph/ops/call_method.rs`), behind a check that the built-ins are
// untouched. That pool is the one engine-specific fast path, and it answers what this file
// answers, by construction.
//
// - `parallelMap`, `parallelFilter`, `parallelFlatMap` and `parallelSort` are `map`, `filter`,
//   `flatMap` and `sort`, applied to the intrinsics captured below when this file runs, so their
//   results, exceptions and side effects are exactly the sequential method's (ADR 0068 rule 3).
//   Nothing is looked up when one is called: `xs.map = f` or `Array.prototype.map = f` afterwards
//   changes `xs.map(...)` and leaves `xs.parallelMap(...)` alone, as it leaves every other
//   built-in that is specified in terms of an abstract operation rather than a property read.
// - `parallelReduce(callback, combine, initial)` has no sequential equivalent, so its meaning is
//   written down once, below, and both engines follow it at every core count (ADR 0069).
// - Each member's `length` is the sequential method's: an optional parameter is written with a
//   default (`thisArg: any = undefined`), which stops `length` counting it.

(function installParallelFamily() {
  if (!kilnExtensions) {
    return;
  }

  // The fixed leaf of `parallelReduce`'s tree. It is part of the method's meaning -- the tree,
  // and so the answer of a combine that is not associative (a floating-point sum), is a function
  // of the input length and this number alone -- and the native lowering uses the same constant
  // (`parallel::REDUCE_LEAF`, held equal to this line by a test in `kiln-builtins`).
  const REDUCE_LEAF = 256;

  // Every intrinsic the family uses, captured once, before any program runs. `Reflect.apply`
  // rather than `.call`, which would read `Function.prototype.call` at each call.
  const apply = Reflect.apply;
  const TypeErrorCtor = TypeError;
  const ObjectCtor = Object;
  const NumberCtor = Number;
  const mathMin = Math.min;
  const mathTrunc = Math.trunc;
  const MAX_SAFE_INTEGER = 9007199254740991;
  const arrayMap = Array.prototype.map;
  const arrayFilter = Array.prototype.filter;
  const arrayFlatMap = Array.prototype.flatMap;
  const arraySort = Array.prototype.sort;

  // One descriptor for every member, not a literal per call: `defineProperty` reads its fields and
  // keeps nothing, and a literal per member was the largest share of the garbage every realm's
  // bootstrap made (realms issue 08).
  const methodDescriptor: PropertyDescriptor = { value: undefined, writable: true, enumerable: false, configurable: true };
  function defineOn(owner: any, name: string, fn: any): void {
    methodDescriptor.value = fn;
    Object.defineProperty(owner, name, methodDescriptor);
    methodDescriptor.value = undefined;
  }
  function defineArrayExtension(name: string, fn: any): void {
    defineOn(Array.prototype, name, fn);
  }

  function toLength(value: any): number {
    const n = NumberCtor(value);
    if (n !== n || n <= 0) return 0;
    if (n === Infinity) return MAX_SAFE_INTEGER;
    return mathMin(mathTrunc(n), MAX_SAFE_INTEGER);
  }

  // `parallelReduce` over `[0, len)` of `O`, which the caller has validated.
  //
  // 1. The indices are cut into leaves of `REDUCE_LEAF`, the last one shorter.
  // 2. Each leaf is folded left to right from `initial`: `acc = callback(acc, O[k], k, O)` for
  //    every `k` in the leaf that `O` has (a hole is skipped, as `reduce` skips it).
  // 3. The leaf results are combined pairwise, left to right, level by level --
  //    `combine(r0, r1), combine(r2, r3), ...`, an odd last one carried up unchanged -- until one
  //    is left.
  //
  // No input: `initial`. One leaf: its fold, and `combine` is never called. `initial` should be
  // an identity of `combine` (0 for a sum) -- every leaf starts from it -- and `combine` should
  // be associative for the answer to read as a plain fold; either way the answer is the one
  // this tree gives, whatever the engine or the core count.
  function reduceTree(O: any, len: number, callback: any, combine: any, initial: any): any {
    // The levels are filled by index rather than `push`, which a program can replace.
    let level: any[] = [];
    let count = 0;
    for (let start = 0; start < len; start += REDUCE_LEAF) {
      const end = mathMin(start + REDUCE_LEAF, len);
      let accumulator = initial;
      for (let k = start; k < end; k++) {
        if (k in O) {
          accumulator = callback(accumulator, O[k], k, O);
        }
      }
      level[count++] = accumulator;
    }
    if (count === 0) {
      return initial;
    }
    while (count > 1) {
      const next: any[] = [];
      let written = 0;
      let i = 0;
      for (; i + 1 < count; i += 2) {
        next[written++] = combine(level[i], level[i + 1]);
      }
      if (i < count) {
        next[written++] = level[i];
      }
      level = next;
      count = written;
    }
    return level[0];
  }

  function requireCallbacks(callback: any, combine: any): void {
    if (typeof callback !== "function") {
      throw new TypeErrorCtor(callback + " is not a function");
    }
    if (typeof combine !== "function") {
      throw new TypeErrorCtor(combine + " is not a function");
    }
  }

  // --- Array.prototype ---

  const parallelMap = { parallelMap(this: any, callback: any, thisArg: any = undefined): any {
    return apply(arrayMap, this, [callback, thisArg]);
  } }.parallelMap;

  const parallelFilter = { parallelFilter(this: any, callback: any, thisArg: any = undefined): any {
    return apply(arrayFilter, this, [callback, thisArg]);
  } }.parallelFilter;

  const parallelFlatMap = { parallelFlatMap(this: any, callback: any, thisArg: any = undefined): any {
    return apply(arrayFlatMap, this, [callback, thisArg]);
  } }.parallelFlatMap;

  const parallelSort = { parallelSort(this: any, compareFn: any): any {
    return apply(arraySort, this, [compareFn]);
  } }.parallelSort;

  const parallelReduce = { parallelReduce(this: any, callback: any, combine: any, initial: any): any {
    if (this === null || this === undefined) {
      throw new TypeErrorCtor("Array.prototype.parallelReduce called on null or undefined");
    }
    const O = ObjectCtor(this);
    const len = toLength(O.length);
    requireCallbacks(callback, combine);
    return reduceTree(O, len, callback, combine, initial);
  } }.parallelReduce;

  defineArrayExtension("parallelMap", parallelMap);
  defineArrayExtension("parallelFilter", parallelFilter);
  defineArrayExtension("parallelFlatMap", parallelFlatMap);
  defineArrayExtension("parallelSort", parallelSort);
  defineArrayExtension("parallelReduce", parallelReduce);

  // --- %TypedArray%.prototype ---

  if (typeof Uint8Array === "undefined" || !Uint8Array.prototype) {
    return;
  }
  const TypedProto = Object.getPrototypeOf(Uint8Array.prototype);
  if (!TypedProto) {
    return;
  }
  function defineTypedArrayExtension(name: string, fn: any): void {
    defineOn(TypedProto, name, fn);
  }
  const typedMap = TypedProto.map;
  const typedFilter = TypedProto.filter;
  const typedSort = TypedProto.sort;
  // `ValidateTypedArray`, borrowed from `at` -- which does nothing else observable -- so the
  // receiver checks are exactly the sequential methods'.
  const typedAt = TypedProto.at;
  if (typeof typedMap !== "function" || typeof typedFilter !== "function" ||
      typeof typedSort !== "function" || typeof typedAt !== "function") {
    // A build that leaves the typed-array family out has no sequential method to borrow.
    return;
  }
  const lengthDescriptor = ObjectCtor.getOwnPropertyDescriptor(TypedProto, "length");
  const lengthGetter: ((this: any) => number) | undefined =
    lengthDescriptor && typeof lengthDescriptor.get === "function" ? lengthDescriptor.get : undefined;

  const typedParallelMap = { parallelMap(this: any, callback: any, thisArg: any = undefined): any {
    return apply(typedMap, this, [callback, thisArg]);
  } }.parallelMap;

  const typedParallelFilter = { parallelFilter(this: any, callback: any, thisArg: any = undefined): any {
    return apply(typedFilter, this, [callback, thisArg]);
  } }.parallelFilter;

  const typedParallelSort = { parallelSort(this: any, compareFn: any): any {
    return apply(typedSort, this, [compareFn]);
  } }.parallelSort;

  const typedParallelReduce = { parallelReduce(this: any, callback: any, combine: any, initial: any): any {
    apply(typedAt, this, [0]);
    const len = lengthGetter !== undefined ? apply(lengthGetter, this, []) : this.length;
    requireCallbacks(callback, combine);
    return reduceTree(this, len, callback, combine, initial);
  } }.parallelReduce;

  defineTypedArrayExtension("parallelMap", typedParallelMap);
  defineTypedArrayExtension("parallelFilter", typedParallelFilter);
  defineTypedArrayExtension("parallelSort", typedParallelSort);
  defineTypedArrayExtension("parallelReduce", typedParallelReduce);
})();
