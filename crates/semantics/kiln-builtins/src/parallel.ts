// kiln-builtins: the parallel collection family (ADR 0068, ADR 0069; parallel-builtins issue 10).
//
// Shared by both interpreter and native execution. These are kiln's own members, declared in
// `kiln_builtin_surface::KILN_EXTENSIONS` and installed only when the realm installs extensions
// (`extensions.ts`), non-enumerable like every built-in method.
//
// This file is the one definition of what each method means, and it is the sequential one. The
// interpreter always runs it. The native backend runs it too, except at a call site whose callback
// it proves pure and numeric (`crates/native/kiln-backend-c/src/parallel.rs`): there it runs the
// same computation on the pool, and the answer is the same by construction.
//
// - `parallelFilter`, `parallelFlatMap` and `parallelSort` are `filter`, `flatMap` and `sort`,
//   called on the intrinsics captured here, so their results, exceptions and side effects are
//   exactly the sequential method's (ADR 0068 rule 3).
// - `parallelReduce(callback, combine, initial)` has no sequential equivalent, so its meaning is
//   written down once, below, and both engines follow it at every core count (ADR 0069).

(function installParallelFamily() {
  if (!kilnExtensions) {
    return;
  }

  // The fixed leaf of `parallelReduce`'s tree. It is part of the method's meaning -- the tree,
  // and so the answer of a combine that is not associative (a floating-point sum), is a function
  // of the input length and this number alone -- and the native lowering uses the same constant
  // (`parallel::REDUCE_LEAF`, held equal to this line by a test in `kiln-builtins`).
  const REDUCE_LEAF = 256;

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
    const n = Number(value);
    if (Number.isNaN(n) || n <= 0) return 0;
    if (!Number.isFinite(n)) return Number.MAX_SAFE_INTEGER;
    return Math.min(Math.trunc(n), Number.MAX_SAFE_INTEGER);
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
    let level: any[] = [];
    for (let start = 0; start < len; start += REDUCE_LEAF) {
      const end = Math.min(start + REDUCE_LEAF, len);
      let accumulator = initial;
      for (let k = start; k < end; k++) {
        if (k in O) {
          accumulator = callback(accumulator, O[k], k, O);
        }
      }
      level.push(accumulator);
    }
    if (level.length === 0) {
      return initial;
    }
    while (level.length > 1) {
      const next: any[] = [];
      let i = 0;
      for (; i + 1 < level.length; i += 2) {
        next.push(combine(level[i], level[i + 1]));
      }
      if (i < level.length) {
        next.push(level[i]);
      }
      level = next;
    }
    return level[0];
  }

  function requireCallbacks(callback: any, combine: any): void {
    if (typeof callback !== "function") {
      throw new TypeError(callback + " is not a function");
    }
    if (typeof combine !== "function") {
      throw new TypeError(combine + " is not a function");
    }
  }

  // --- Array.prototype ---

  const parallelFilter = { parallelFilter(this: any, callback: any, thisArg: any = undefined): any {
    return arrayFilter.call(this, callback, thisArg);
  } }.parallelFilter;

  const parallelFlatMap = { parallelFlatMap(this: any, callback: any, thisArg: any = undefined): any {
    return arrayFlatMap.call(this, callback, thisArg);
  } }.parallelFlatMap;

  const parallelSort = { parallelSort(this: any, compareFn: any): any {
    return arraySort.call(this, compareFn);
  } }.parallelSort;

  const parallelReduce = { parallelReduce(this: any, callback: any, combine: any, initial: any): any {
    if (this === null || this === undefined) {
      throw new TypeError("Array.prototype.parallelReduce called on null or undefined");
    }
    const O = Object(this);
    const len = toLength(O.length);
    requireCallbacks(callback, combine);
    return reduceTree(O, len, callback, combine, initial);
  } }.parallelReduce;

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
  const typedFilter = TypedProto.filter;
  const typedSort = TypedProto.sort;
  // `ValidateTypedArray`, borrowed from `at` -- which does nothing else observable -- so the
  // receiver checks are exactly the sequential methods'.
  const typedAt = TypedProto.at;
  const lengthDescriptor = Object.getOwnPropertyDescriptor(TypedProto, "length");
  const lengthGetter: ((this: any) => number) | undefined =
    lengthDescriptor && typeof lengthDescriptor.get === "function" ? lengthDescriptor.get : undefined;

  const typedParallelFilter = { parallelFilter(this: any, callback: any, thisArg: any = undefined): any {
    return typedFilter.call(this, callback, thisArg);
  } }.parallelFilter;

  const typedParallelSort = { parallelSort(this: any, compareFn: any): any {
    return typedSort.call(this, compareFn);
  } }.parallelSort;

  const typedParallelReduce = { parallelReduce(this: any, callback: any, combine: any, initial: any): any {
    typedAt.call(this, 0);
    const len = lengthGetter !== undefined ? lengthGetter.call(this) : this.length;
    requireCallbacks(callback, combine);
    return reduceTree(this, len, callback, combine, initial);
  } }.parallelReduce;

  defineTypedArrayExtension("parallelFilter", typedParallelFilter);
  defineTypedArrayExtension("parallelSort", typedParallelSort);
  defineTypedArrayExtension("parallelReduce", typedParallelReduce);
})();
