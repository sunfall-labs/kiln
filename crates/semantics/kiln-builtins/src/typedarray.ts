// kiln-builtins: `%TypedArray%.prototype` (ADR 0005, ADR 0063 rule 4, Phase 5.1).
//
// Shared by both interpreter and native execution.
// Implements the complete %TypedArray%.prototype specification in TypeScript.

// `IsTypedArrayOutOfBounds` (ADR 0065, representation): whether a view's window has left its
// buffer -- detached, or a resizable buffer shrunk under it. It reads the view's own offset and
// length, which no public accessor reports once the view is out of bounds (both answer 0). The
// native backend has no resizable buffers, so there a view is out of bounds exactly when it is
// detached, and it publishes none.
const __kilnTypedArrayOutOfBounds: ((view: any) => boolean) | undefined =
  (globalThis as any).__kilnTypedArrayOutOfBounds;
delete (globalThis as any).__kilnTypedArrayOutOfBounds;

// Element storage (ADR 0065, representation), which both engines publish for this file alone.
//
// `__kilnTypedArrayCopy(target, targetIndex, source, sourceIndex, count)` copies `count` elements
// between two views of one element type as bytes, as if through a temporary -- the specification's
// own step for a same-type `slice`, `set` and `copyWithin` -- and answers `true`. Two element types,
// a range past either view's current length or a detached buffer answer `false` with nothing
// written, and the element loop beside each call runs instead.
//
// `__kilnTypedArraySort(view)` sorts a view's elements into the default order (`-0` before `+0`,
// `NaN` last) in place and answers `true`: the order `kiln_number_core::typed_order` owns for both
// engines. `false` (a detached view, or scratch the engine could not allocate) writes nothing, and
// `sortItems` below sorts instead.
//
// Measured on a 1M-element `Float64Array`, native (parallel issue 16): `slice` 112 ms and the
// default `sort` 2.5 s as element loops here, against node's 2.6 ms and 84 ms.
const __kilnTypedArrayCopy: (target: any, targetIndex: number, source: any, sourceIndex: number, count: number) => boolean =
  (globalThis as any).__kilnTypedArrayCopy;
delete (globalThis as any).__kilnTypedArrayCopy;
const __kilnTypedArraySort: (view: any) => boolean = (globalThis as any).__kilnTypedArraySort;
delete (globalThis as any).__kilnTypedArraySort;

(function installTypedArrayPrototype() {
  if (typeof Uint8Array === "undefined" || !Uint8Array.prototype) {
    return;
  }

  const Proto = Object.getPrototypeOf(Uint8Array.prototype);
  if (!Proto) {
    return;
  }

  // `[[ArrayLength]]`, read through the realm's own `length` getter so that a `length` a program
  // defines on an instance or a subclass prototype cannot change what a method walks. The native
  // backend publishes no accessor there -- its `.length` on a view is already the element count --
  // so it reads the property.
  const lengthDescriptor = Object.getOwnPropertyDescriptor(Proto, "length");
  const lengthGetter: ((this: any) => number) | undefined =
    lengthDescriptor && typeof lengthDescriptor.get === "function" ? lengthDescriptor.get : undefined;
  function lengthOf(ta: any): number {
    return lengthGetter !== undefined ? lengthGetter.call(ta) : ta.length;
  }

  function requireTypedArray(thisArg: any, methodName: string): any {
    if (
      thisArg === null ||
      (typeof thisArg !== "object" && typeof thisArg !== "function") ||
      !ArrayBuffer.isView(thisArg) ||
      (typeof DataView === "function" && thisArg instanceof DataView) ||
      !("byteLength" in thisArg) ||
      !("buffer" in thisArg)
    ) {
      throw new TypeError(`Method %TypedArray%.prototype.${methodName} called on incompatible receiver`);
    }
    return thisArg;
  }

  function isDetached(ta: any): boolean {
    try {
      const buf = ta.buffer;
      if (buf && (buf as any).detached === true) {
        return true;
      }
    } catch {
      return true;
    }
    return false;
  }

  function isOutOfBounds(ta: any): boolean {
    return __kilnTypedArrayOutOfBounds !== undefined ? __kilnTypedArrayOutOfBounds(ta) : isDetached(ta);
  }

  // `ValidateTypedArray`: a view, and one whose window is still inside its buffer.
  function validateTypedArray(thisArg: any, methodName: string): any {
    requireTypedArray(thisArg, methodName);
    if (isOutOfBounds(thisArg)) {
      throw new TypeError(`Method %TypedArray%.prototype.${methodName} called on a detached or out-of-bounds ArrayBuffer`);
    }
    return thisArg;
  }

  function toIntegerOrInfinity(val: any): number {
    const n = Number(val);
    if (Number.isNaN(n) || n === 0) return 0;
    if (!Number.isFinite(n)) return n;
    return Math.trunc(n);
  }

  function toLength(val: any): number {
    const len = toIntegerOrInfinity(val);
    if (len <= 0) return 0;
    return Math.min(len, 0x1fffffffffffff);
  }

  function isBigIntArray(ta: any): boolean {
    if (typeof BigInt64Array === "function" && ta instanceof BigInt64Array) return true;
    if (typeof BigUint64Array === "function" && ta instanceof BigUint64Array) return true;
    const tag = (typeof Symbol !== "undefined" && Symbol.toStringTag && (ta as any)[Symbol.toStringTag]) || "";
    return tag === "BigInt64Array" || tag === "BigUint64Array";
  }

  function getIntrinsicConstructor(ta: any): any {
    if (typeof Symbol !== "undefined" && Symbol.toStringTag) {
      const tag = (ta as any)[Symbol.toStringTag];
      if (tag && typeof (globalThis as any)[tag] === "function") {
        return (globalThis as any)[tag];
      }
    }
    if (typeof ta.constructor === "function") {
      return ta.constructor;
    }
    return Uint8Array;
  }

  function getSpeciesConstructor(ta: any, defaultConstructor: any): any {
    const ctor = ta.constructor;
    if (ctor === undefined) return defaultConstructor;
    if (ctor === null || (typeof ctor !== "object" && typeof ctor !== "function")) {
      throw new TypeError("Invalid constructor");
    }
    if (typeof Symbol !== "undefined" && Symbol.species) {
      const species = ctor[Symbol.species];
      if (species === null || species === undefined) {
        return defaultConstructor;
      }
      if (typeof species !== "function") {
        throw new TypeError("Species is not a constructor");
      }
      return species;
    }
    return defaultConstructor;
  }

  function typedArraySpeciesCreate(exemplar: any, length: number, methodName: string): any {
    const defaultCtor = getIntrinsicConstructor(exemplar);
    const ctor = getSpeciesConstructor(exemplar, defaultCtor);
    const result = new ctor(length);
    requireTypedArray(result, methodName);
    if (isDetached(result)) {
      throw new TypeError("Constructed TypedArray buffer is detached");
    }
    if (isBigIntArray(exemplar) !== isBigIntArray(result)) {
      throw new TypeError("Cannot mix BigInt and other types, use explicit conversions");
    }
    return result;
  }

  function typedArrayCreateSameType(exemplar: any, length: number): any {
    const ctor = getIntrinsicConstructor(exemplar);
    return new ctor(length);
  }

  // --- Search ---

  const indexOf = { indexOf(this: any, searchElement: any, fromIndex: any = undefined): number {
    validateTypedArray(this, "indexOf");
    const len = lengthOf(this);
    if (len === 0) return -1;
    const n = fromIndex === undefined ? 0 : toIntegerOrInfinity(fromIndex);
    if (n === Infinity) return -1;

    let k: number;
    if (n === -Infinity) {
      k = 0;
    } else if (n >= 0) {
      k = n;
    } else {
      k = len + n;
      if (k < 0) k = 0;
    }

    const isBigInt = isBigIntArray(this);
    if (isBigInt && typeof searchElement !== "bigint") return -1;
    if (!isBigInt && typeof searchElement === "bigint") return -1;

    while (k < len) {
      if (this[k] === searchElement) {
        return k;
      }
      k++;
    }
    return -1;
  } }.indexOf;

  const lastIndexOf = { lastIndexOf(this: any, searchElement: any, fromIndex: any = undefined): number {
    validateTypedArray(this, "lastIndexOf");
    const len = lengthOf(this);
    if (len === 0) return -1;
    // "If fromIndex is present": an explicit `undefined` is present, and converts to 0.
    const n = arguments.length < 2 ? len - 1 : toIntegerOrInfinity(fromIndex);
    if (n === -Infinity) return -1;

    let k: number;
    if (n >= 0) {
      k = Math.min(n, len - 1);
    } else {
      k = len + n;
    }

    const isBigInt = isBigIntArray(this);
    if (isBigInt && typeof searchElement !== "bigint") return -1;
    if (!isBigInt && typeof searchElement === "bigint") return -1;

    while (k >= 0) {
      if (this[k] === searchElement) {
        return k;
      }
      k--;
    }
    return -1;
  } }.lastIndexOf;

  const includes = { includes(this: any, searchElement: any, fromIndex: any = undefined): boolean {
    validateTypedArray(this, "includes");
    const len = lengthOf(this);
    if (len === 0) return false;
    const n = fromIndex === undefined ? 0 : toIntegerOrInfinity(fromIndex);
    if (n === Infinity) return false;

    let k: number;
    if (n === -Infinity) {
      k = 0;
    } else if (n >= 0) {
      k = n;
    } else {
      k = len + n;
      if (k < 0) k = 0;
    }

    const isBigInt = isBigIntArray(this);
    if (isBigInt && typeof searchElement !== "bigint") return false;
    if (!isBigInt && typeof searchElement === "bigint") return false;

    const searchIsNaN = typeof searchElement === "number" && Number.isNaN(searchElement);

    while (k < len) {
      const element = this[k];
      if (searchIsNaN) {
        if (typeof element === "number" && Number.isNaN(element)) {
          return true;
        }
      } else if (element === searchElement) {
        return true;
      }
      k++;
    }
    return false;
  } }.includes;

  // --- Predicates / Searchers ---

  const find = { find(this: any, predicate: any, thisArg: any = undefined): any {
    validateTypedArray(this, "find");
    if (typeof predicate !== "function") {
      throw new TypeError(predicate + " is not a function");
    }
    const len = lengthOf(this);
    for (let k = 0; k < len; k++) {
      const kValue = this[k];
      if (thisArg === undefined ? predicate(kValue, k, this) : predicate.call(thisArg, kValue, k, this)) {
        return kValue;
      }
    }
    return undefined;
  } }.find;

  const findIndex = { findIndex(this: any, predicate: any, thisArg: any = undefined): number {
    validateTypedArray(this, "findIndex");
    if (typeof predicate !== "function") {
      throw new TypeError(predicate + " is not a function");
    }
    const len = lengthOf(this);
    for (let k = 0; k < len; k++) {
      const kValue = this[k];
      if (thisArg === undefined ? predicate(kValue, k, this) : predicate.call(thisArg, kValue, k, this)) {
        return k;
      }
    }
    return -1;
  } }.findIndex;

  const findLast = { findLast(this: any, predicate: any, thisArg: any = undefined): any {
    validateTypedArray(this, "findLast");
    if (typeof predicate !== "function") {
      throw new TypeError(predicate + " is not a function");
    }
    const len = lengthOf(this);
    for (let k = len - 1; k >= 0; k--) {
      const kValue = this[k];
      if (thisArg === undefined ? predicate(kValue, k, this) : predicate.call(thisArg, kValue, k, this)) {
        return kValue;
      }
    }
    return undefined;
  } }.findLast;

  const findLastIndex = { findLastIndex(this: any, predicate: any, thisArg: any = undefined): number {
    validateTypedArray(this, "findLastIndex");
    if (typeof predicate !== "function") {
      throw new TypeError(predicate + " is not a function");
    }
    const len = lengthOf(this);
    for (let k = len - 1; k >= 0; k--) {
      const kValue = this[k];
      if (thisArg === undefined ? predicate(kValue, k, this) : predicate.call(thisArg, kValue, k, this)) {
        return k;
      }
    }
    return -1;
  } }.findLastIndex;

  const some = { some(this: any, predicate: any, thisArg: any = undefined): boolean {
    validateTypedArray(this, "some");
    if (typeof predicate !== "function") {
      throw new TypeError(predicate + " is not a function");
    }
    const len = lengthOf(this);
    for (let k = 0; k < len; k++) {
      if (thisArg === undefined ? predicate(this[k], k, this) : predicate.call(thisArg, this[k], k, this)) {
        return true;
      }
    }
    return false;
  } }.some;

  const every = { every(this: any, predicate: any, thisArg: any = undefined): boolean {
    validateTypedArray(this, "every");
    if (typeof predicate !== "function") {
      throw new TypeError(predicate + " is not a function");
    }
    const len = lengthOf(this);
    for (let k = 0; k < len; k++) {
      if (!(thisArg === undefined ? predicate(this[k], k, this) : predicate.call(thisArg, this[k], k, this))) {
        return false;
      }
    }
    return true;
  } }.every;

  // --- Reducers / Iterators ---

  // Every callback below is `Call(callback, thisArg, args)`. With no thisArg that is a plain call
  // (`this` undefined, as `.call(undefined, ...)` would give it): the spelling that does not read
  // `Function.prototype.call` -- which a program may have replaced -- and, natively, the one that
  // does not go through a method dispatch per element.
  const forEach = { forEach(this: any, callback: any, thisArg: any = undefined): void {
    validateTypedArray(this, "forEach");
    if (typeof callback !== "function") {
      throw new TypeError(callback + " is not a function");
    }
    const len = lengthOf(this);
    for (let k = 0; k < len; k++) {
      if (thisArg === undefined) {
        callback(this[k], k, this);
      } else {
        callback.call(thisArg, this[k], k, this);
      }
    }
  } }.forEach;

  const map = { map(this: any, callback: any, thisArg: any = undefined): any {
    validateTypedArray(this, "map");
    if (typeof callback !== "function") {
      throw new TypeError(callback + " is not a function");
    }
    const len = lengthOf(this);
    const result = typedArraySpeciesCreate(this, len, "map");
    for (let k = 0; k < len; k++) {
      result[k] = thisArg === undefined ? callback(this[k], k, this) : callback.call(thisArg, this[k], k, this);
    }
    return result;
  } }.map;

  const filter = { filter(this: any, callback: any, thisArg: any = undefined): any {
    validateTypedArray(this, "filter");
    if (typeof callback !== "function") {
      throw new TypeError(callback + " is not a function");
    }
    const len = lengthOf(this);
    const kept: any[] = [];
    for (let k = 0; k < len; k++) {
      const val = this[k];
      if (thisArg === undefined ? callback(val, k, this) : callback.call(thisArg, val, k, this)) {
        kept.push(val);
      }
    }
    const result = typedArraySpeciesCreate(this, kept.length, "filter");
    for (let n = 0; n < kept.length; n++) {
      result[n] = kept[n];
    }
    return result;
  } }.filter;

  const reduce = { reduce(this: any, callback: any, initialValue: any = undefined): any {
    validateTypedArray(this, "reduce");
    if (typeof callback !== "function") {
      throw new TypeError(callback + " is not a function");
    }
    const len = lengthOf(this);
    if (len === 0 && arguments.length < 2) {
      throw new TypeError("Reduce of empty array with no initial value");
    }
    let k = 0;
    let accumulator: any;
    if (arguments.length >= 2) {
      accumulator = initialValue;
    } else {
      accumulator = this[0];
      k = 1;
    }
    while (k < len) {
      accumulator = callback(accumulator, this[k], k, this);
      k++;
    }
    return accumulator;
  } }.reduce;

  const reduceRight = { reduceRight(this: any, callback: any, initialValue: any = undefined): any {
    validateTypedArray(this, "reduceRight");
    if (typeof callback !== "function") {
      throw new TypeError(callback + " is not a function");
    }
    const len = lengthOf(this);
    if (len === 0 && arguments.length < 2) {
      throw new TypeError("Reduce of empty array with no initial value");
    }
    let k = len - 1;
    let accumulator: any;
    if (arguments.length >= 2) {
      accumulator = initialValue;
    } else {
      accumulator = this[k];
      k--;
    }
    while (k >= 0) {
      accumulator = callback(accumulator, this[k], k, this);
      k--;
    }
    return accumulator;
  } }.reduceRight;

  const arrayJoin = Array.prototype.join;
  const join = { join(this: any, separator?: any): string {
    validateTypedArray(this, "join");
    const len = lengthOf(this);
    const sep = separator === undefined ? "," : String(separator);
    // The pieces, joined once: appending to one string per element copied the whole prefix each
    // time natively (5.2 s for 100k elements, against node's 6.5 ms).
    const parts: string[] = [];
    for (let k = 0; k < len; k++) {
      const el = this[k];
      parts.push(el !== undefined && el !== null ? String(el) : "");
    }
    return arrayJoin.call(parts, sep);
  } }.join;

  function createTypedArrayIterator(ta: any, kind: "entries" | "keys" | "values"): any {
    let index = 0;
    const iterator = {
      next() {
        if (isDetached(ta)) {
          throw new TypeError("Cannot iterate over detached ArrayBuffer");
        }
        if (index >= lengthOf(ta)) {
          return { value: undefined, done: true };
        }
        const i = index++;
        let value: any;
        if (kind === "keys") {
          value = i;
        } else if (kind === "values") {
          value = ta[i];
        } else {
          value = [i, ta[i]];
        }
        return { value, done: false };
      },
      [Symbol.iterator]() {
        return this;
      },
    };
    if (typeof Symbol !== "undefined" && Symbol.toStringTag) {
      Object.defineProperty(iterator, Symbol.toStringTag, {
        value: "Array Iterator",
        configurable: true,
        writable: false,
        enumerable: false,
      });
    }
    return iterator;
  }

  // `CreateArrayIterator` over a view: the realm's own Array Iterator, whose prototype is
  // %ArrayIteratorPrototype% and whose `next` already knows a view's length -- and that it throws
  // once the view is out of bounds -- so the view gets the object an array gets.
  const arrayEntries = Array.prototype.entries;
  const arrayKeys = Array.prototype.keys;
  const arrayValues = Array.prototype.values;

  const entries = { entries(this: any): any {
    validateTypedArray(this, "entries");
    return typeof arrayEntries === "function" ? arrayEntries.call(this) : createTypedArrayIterator(this, "entries");
  } }.entries;

  const keys = { keys(this: any): any {
    validateTypedArray(this, "keys");
    return typeof arrayKeys === "function" ? arrayKeys.call(this) : createTypedArrayIterator(this, "keys");
  } }.keys;

  const values = { values(this: any): any {
    validateTypedArray(this, "values");
    return typeof arrayValues === "function" ? arrayValues.call(this) : createTypedArrayIterator(this, "values");
  } }.values;

  // --- Mutators / Copiers ---

  const slice = { slice(this: any, start: any, end: any): any {
    validateTypedArray(this, "slice");
    const len = lengthOf(this);
    const relativeStart = start === undefined ? 0 : toIntegerOrInfinity(start);
    let k: number;
    if (relativeStart === -Infinity) {
      k = 0;
    } else if (relativeStart < 0) {
      k = Math.max(len + relativeStart, 0);
    } else {
      k = Math.min(relativeStart, len);
    }

    const relativeEnd = end === undefined ? len : toIntegerOrInfinity(end);
    let final: number;
    if (relativeEnd === -Infinity) {
      final = 0;
    } else if (relativeEnd < 0) {
      final = Math.max(len + relativeEnd, 0);
    } else {
      final = Math.min(relativeEnd, len);
    }

    const count = Math.max(final - k, 0);
    const result = typedArraySpeciesCreate(this, count, "slice");
    if (count > 0) {
      if (isDetached(this)) {
        throw new TypeError("Method %TypedArray%.prototype.slice called on detached ArrayBuffer");
      }
      // A species constructor could hand back a view of this very buffer, which the
      // specification copies front to back, byte by byte: only a separate buffer is a plain copy.
      if (result.buffer === this.buffer || !__kilnTypedArrayCopy(result, 0, this, k, count)) {
        for (let n = 0; n < count; n++, k++) {
          result[n] = this[k];
        }
      }
    }
    return result;
  } }.slice;

  const subarray = { subarray(this: any, begin: any, end: any): any {
    requireTypedArray(this, "subarray");
    const buffer = this.buffer;
    const srcLength = lengthOf(this);
    const relativeBegin = begin === undefined ? 0 : toIntegerOrInfinity(begin);
    let startIndex: number;
    if (relativeBegin === -Infinity) {
      startIndex = 0;
    } else if (relativeBegin < 0) {
      startIndex = Math.max(srcLength + relativeBegin, 0);
    } else {
      startIndex = Math.min(relativeBegin, srcLength);
    }

    const relativeEnd = end === undefined ? srcLength : toIntegerOrInfinity(end);
    let endIndex: number;
    if (relativeEnd === -Infinity) {
      endIndex = 0;
    } else if (relativeEnd < 0) {
      endIndex = Math.max(srcLength + relativeEnd, 0);
    } else {
      endIndex = Math.min(relativeEnd, srcLength);
    }

    const newLength = Math.max(endIndex - startIndex, 0);
    const defaultCtor = getIntrinsicConstructor(this);
    const constructor = getSpeciesConstructor(this, defaultCtor);
    const elementSize = this.BYTES_PER_ELEMENT || 1;
    const srcByteOffset = this.byteOffset;
    const beginByteOffset = srcByteOffset + startIndex * elementSize;
    return new constructor(buffer, beginByteOffset, newLength);
  } }.subarray;

  const set = { set(this: any, source: any, offset: any = undefined): void {
    requireTypedArray(this, "set");
    const targetOffset = offset === undefined ? 0 : toIntegerOrInfinity(offset);
    if (targetOffset < 0) {
      throw new RangeError("offset is out of bounds");
    }
    if (isDetached(this)) {
      throw new TypeError("Method %TypedArray%.prototype.set called on detached ArrayBuffer");
    }
    if (source === null || source === undefined) {
      throw new TypeError("Cannot convert undefined or null to object");
    }

    const targetLength = lengthOf(this);
    const isView = ArrayBuffer.isView(source) && !(typeof DataView === "function" && source instanceof DataView);

    if (isView) {
      if (isDetached(source)) {
        throw new TypeError("Source ArrayBuffer is detached");
      }
      const srcLength = lengthOf(source);
      if (targetOffset + srcLength > targetLength || targetOffset > targetLength) {
        throw new RangeError("offset is out of bounds");
      }
      if (isBigIntArray(this) !== isBigIntArray(source)) {
        throw new TypeError("Cannot mix BigInt and other types, use explicit conversions");
      }
      if (__kilnTypedArrayCopy(this, targetOffset, source, 0, srcLength)) {
        return;
      }
      if (this.buffer === (source as any).buffer) {
        const temp = new (source.constructor)(source);
        for (let i = 0; i < srcLength; i++) {
          this[targetOffset + i] = temp[i];
        }
      } else {
        for (let i = 0; i < srcLength; i++) {
          this[targetOffset + i] = (source as any)[i];
        }
      }
    } else {
      const src = Object(source);
      const srcLength = toLength(src.length);
      if (targetOffset + srcLength > targetLength || targetOffset > targetLength) {
        throw new RangeError("offset is out of bounds");
      }
      for (let i = 0; i < srcLength; i++) {
        this[targetOffset + i] = src[i];
      }
    }
  } }.set;

  const copyWithin = { copyWithin(this: any, target: any, start: any, end: any = undefined): any {
    validateTypedArray(this, "copyWithin");
    const len = lengthOf(this);
    const relativeTarget = toIntegerOrInfinity(target);
    let to: number;
    if (relativeTarget === -Infinity) {
      to = 0;
    } else if (relativeTarget < 0) {
      to = Math.max(len + relativeTarget, 0);
    } else {
      to = Math.min(relativeTarget, len);
    }

    const relativeStart = toIntegerOrInfinity(start);
    let from: number;
    if (relativeStart === -Infinity) {
      from = 0;
    } else if (relativeStart < 0) {
      from = Math.max(len + relativeStart, 0);
    } else {
      from = Math.min(relativeStart, len);
    }

    const relativeEnd = end === undefined ? len : toIntegerOrInfinity(end);
    let final: number;
    if (relativeEnd === -Infinity) {
      final = 0;
    } else if (relativeEnd < 0) {
      final = Math.max(len + relativeEnd, 0);
    } else {
      final = Math.min(relativeEnd, len);
    }

    let count = Math.min(final - from, len - to);
    if (count > 0 && !__kilnTypedArrayCopy(this, to, this, from, count)) {
      let direction = 1;
      if (from < to && to < from + count) {
        direction = -1;
        from = from + count - 1;
        to = to + count - 1;
      }
      while (count > 0) {
        this[to] = this[from];
        from += direction;
        to += direction;
        count--;
      }
    }
    return this;
  } }.copyWithin;

  const fill = { fill(this: any, value: any, start: any = undefined, end: any = undefined): any {
    validateTypedArray(this, "fill");
    const len = lengthOf(this);
    const isBigInt = isBigIntArray(this);
    const numericValue = isBigInt ? BigInt(value) : Number(value);

    const relativeStart = start === undefined ? 0 : toIntegerOrInfinity(start);
    let k: number;
    if (relativeStart === -Infinity) {
      k = 0;
    } else if (relativeStart < 0) {
      k = Math.max(len + relativeStart, 0);
    } else {
      k = Math.min(relativeStart, len);
    }

    const relativeEnd = end === undefined ? len : toIntegerOrInfinity(end);
    let final: number;
    if (relativeEnd === -Infinity) {
      final = 0;
    } else if (relativeEnd < 0) {
      final = Math.max(len + relativeEnd, 0);
    } else {
      final = Math.min(relativeEnd, len);
    }

    if (isDetached(this)) {
      throw new TypeError("Method %TypedArray%.prototype.fill called on detached ArrayBuffer");
    }

    // One element converted and written, then doubled by byte copies -- the same bytes the
    // loop below writes into every element, which it still does for whatever a copy declined.
    if (k < final) {
      this[k] = numericValue;
      let filled = 1;
      while (k + filled < final) {
        const n = Math.min(filled, final - k - filled);
        if (!__kilnTypedArrayCopy(this, k + filled, this, k, n)) break;
        filled += n;
      }
      k += filled;
    }
    while (k < final) {
      this[k] = numericValue;
      k++;
    }
    return this;
  } }.fill;

  const reverse = { reverse(this: any): any {
    validateTypedArray(this, "reverse");
    const len = lengthOf(this);
    const middle = Math.floor(len / 2);
    let lower = 0;
    while (lower !== middle) {
      const upper = len - lower - 1;
      const lowerVal = this[lower];
      const upperVal = this[upper];
      this[lower] = upperVal;
      this[upper] = lowerVal;
      lower++;
    }
    return this;
  } }.reverse;

  // `SortIndexedProperties` over `items`, a copy of a view's elements, with
  // `TypedArraySortCompare`: the comparator's answer through `ToNumber` (`NaN` as `+0`), or with
  // none the default order -- which `__kilnTypedArraySort` runs on the view itself, so this sorts
  // by it only when that primitive declined.
  function sortItems(items: any[], compareFn: any, isBigInt: boolean): void {
    if (compareFn !== undefined) {
      // `Array.prototype.sort`'s `SortCompare` is this one for elements that are never
      // `undefined`: `ToNumber` of the comparator's answer, `NaN` read as `+0`.
      items.sort(compareFn);
    } else if (isBigInt) {
      items.sort((x: bigint, y: bigint) => {
        if (x < y) return -1;
        if (x > y) return 1;
        return 0;
      });
    } else {
      items.sort((x: number, y: number) => {
        const isXNaN = Number.isNaN(x);
        const isYNaN = Number.isNaN(y);
        if (isXNaN && isYNaN) return 0;
        if (isXNaN) return 1;
        if (isYNaN) return -1;
        if (x < y) return -1;
        if (x > y) return 1;
        if (x === 0 && y === 0) {
          const xSign = 1 / x;
          const ySign = 1 / y;
          if (xSign < ySign) return -1;
          if (xSign > ySign) return 1;
          return 0;
        }
        return 0;
      });
    }
  }

  const sort = { sort(this: any, compareFn?: any): any {
    validateTypedArray(this, "sort");
    if (compareFn !== undefined && typeof compareFn !== "function") {
      throw new TypeError("The comparison function must be either a function or undefined");
    }

    const len = lengthOf(this);
    if (len <= 1) return this;
    if (compareFn === undefined && __kilnTypedArraySort(this)) {
      return this;
    }

    const items: any[] = [];
    for (let i = 0; i < len; i++) {
      items.push(this[i]);
    }
    sortItems(items, compareFn, isBigIntArray(this));

    if (isDetached(this)) {
      throw new TypeError("Method %TypedArray%.prototype.sort called on detached ArrayBuffer");
    }

    for (let i = 0; i < len; i++) {
      this[i] = items[i];
    }

    return this;
  } }.sort;

  // --- ES2023 change-by-copy ---

  const at = { at(this: any, index: any): any {
    validateTypedArray(this, "at");
    const len = lengthOf(this);
    const relativeIndex = toIntegerOrInfinity(index);
    const k = relativeIndex >= 0 ? relativeIndex : len + relativeIndex;
    if (k < 0 || k >= len) {
      return undefined;
    }
    return this[k];
  } }.at;

  const toReversed = { toReversed(this: any): any {
    validateTypedArray(this, "toReversed");
    const len = lengthOf(this);
    const result = typedArrayCreateSameType(this, len);
    for (let i = 0; i < len; i++) {
      result[i] = this[len - 1 - i];
    }
    return result;
  } }.toReversed;

  const toSorted = { toSorted(this: any, compareFn?: any): any {
    validateTypedArray(this, "toSorted");
    if (compareFn !== undefined && typeof compareFn !== "function") {
      throw new TypeError("The comparison function must be either a function or undefined");
    }

    const len = lengthOf(this);
    const result = typedArrayCreateSameType(this, len);
    if (
      compareFn === undefined &&
      __kilnTypedArrayCopy(result, 0, this, 0, len) &&
      __kilnTypedArraySort(result)
    ) {
      return result;
    }
    const items: any[] = [];
    for (let i = 0; i < len; i++) {
      items.push(this[i]);
    }
    sortItems(items, compareFn, isBigIntArray(this));

    for (let i = 0; i < len; i++) {
      result[i] = items[i];
    }
    return result;
  } }.toSorted;

  const withMethod = { with(this: any, index: any, value: any): any {
    validateTypedArray(this, "with");
    const len = lengthOf(this);
    const relativeIndex = toIntegerOrInfinity(index);
    const actualIndex = relativeIndex >= 0 ? relativeIndex : len + relativeIndex;

    const isBigInt = isBigIntArray(this);
    const numericValue = isBigInt ? BigInt(value) : Number(value);

    if (actualIndex < 0 || actualIndex >= len) {
      throw new RangeError("Invalid typed array index");
    }

    const result = typedArrayCreateSameType(this, len);
    for (let k = 0; k < len; k++) {
      result[k] = k === actualIndex ? numericValue : this[k];
    }
    return result;
  } }.with;
  Object.defineProperty(withMethod, "name", { value: "with", configurable: true });

  // --- String conversions and extensions ---

  function toStringMethod(this: any): string {
    if (typeof this?.join === "function") {
      return this.join();
    }
    return Object.prototype.toString.call(this);
  }

  const toLocaleString = { toLocaleString(this: any, locales: any = undefined, options: any = undefined): string {
    validateTypedArray(this, "toLocaleString");
    const len = lengthOf(this);
    let res = "";
    for (let k = 0; k < len; k++) {
      if (k > 0) res += ",";
      const el = this[k];
      if (el !== undefined && el !== null) {
        if (typeof el.toLocaleString === "function") {
          res += el.toLocaleString(locales, options);
        } else {
          res += String(el);
        }
      }
    }
    return res;
  } }.toLocaleString;

  function parallelMap(this: any, callback: any, thisArg?: any): any {
    requireTypedArray(this, "parallelMap");
    return this.map(callback, thisArg);
  }

  // --- Method installation ---

  // Built-in methods are not constructors, so every one is a method definition (`{ m() {} }.m`)
  // rather than a function declaration. An optional parameter is written with a default
  // (`x: any = undefined`), which stops `length` counting it, as the specification's lengths
  // require; both engines take `length` from the signature.
  function defineMethod(name: string | symbol, fn: any): void {
    Object.defineProperty(Proto, name, {
      value: fn,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  }

  // In the order node's `Reflect.ownKeys(%TypedArray%.prototype)` reports them: key order is
  // observable, and a program that lists the prototype should see the same list in every engine.
  defineMethod("entries", entries);
  defineMethod("keys", keys);
  defineMethod("values", values);
  defineMethod("at", at);
  defineMethod("copyWithin", copyWithin);
  defineMethod("every", every);
  defineMethod("fill", fill);
  defineMethod("filter", filter);
  defineMethod("find", find);
  defineMethod("findIndex", findIndex);
  defineMethod("findLast", findLast);
  defineMethod("findLastIndex", findLastIndex);
  defineMethod("forEach", forEach);
  defineMethod("includes", includes);
  defineMethod("indexOf", indexOf);
  defineMethod("join", join);
  defineMethod("lastIndexOf", lastIndexOf);
  defineMethod("map", map);
  defineMethod("reverse", reverse);
  defineMethod("reduce", reduce);
  defineMethod("reduceRight", reduceRight);
  defineMethod("set", set);
  defineMethod("slice", slice);
  defineMethod("some", some);
  defineMethod("sort", sort);
  defineMethod("subarray", subarray);
  defineMethod("toReversed", toReversed);
  defineMethod("toSorted", toSorted);
  defineMethod("with", withMethod);
  defineMethod("toLocaleString", toLocaleString);
  defineMethod(
    "toString",
    typeof Array.prototype.toString === "function" ? Array.prototype.toString : toStringMethod
  );
  if (typeof Symbol !== "undefined" && Symbol.iterator) {
    defineMethod(Symbol.iterator, values);
  }

  // kiln's own members (ADR 0068): declared in `kiln_builtin_surface::KILN_EXTENSIONS`, installed
  // only when the realm installs extensions (`extensions.ts`), and non-enumerable like the rest.
  function defineExtension(name: string, fn: any): void {
    if (kilnExtensions) {
      defineMethod(name, fn);
    }
  }
  defineExtension("parallelMap", parallelMap);
})();
