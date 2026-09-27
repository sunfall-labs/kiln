// kiln-builtins: `%TypedArray%.prototype` (ADR 0005, ADR 0063 rule 4, Phase 5.1).
//
// Shared by both interpreter and native execution.
// Implements the complete %TypedArray%.prototype specification in TypeScript.

(function installTypedArrayPrototype() {
  if (typeof Uint8Array === "undefined" || !Uint8Array.prototype) {
    return;
  }

  const Proto = Object.getPrototypeOf(Uint8Array.prototype);
  if (!Proto) {
    return;
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

  function validateTypedArray(thisArg: any, methodName: string): any {
    requireTypedArray(thisArg, methodName);
    if (isDetached(thisArg)) {
      throw new TypeError(`Method %TypedArray%.prototype.${methodName} called on detached ArrayBuffer`);
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

  function indexOf(this: any, searchElement: any, fromIndex?: any): number {
    validateTypedArray(this, "indexOf");
    const len = this.length;
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
  }

  function lastIndexOf(this: any, searchElement: any, fromIndex?: any): number {
    validateTypedArray(this, "lastIndexOf");
    const len = this.length;
    if (len === 0) return -1;
    const n = fromIndex === undefined ? len - 1 : toIntegerOrInfinity(fromIndex);
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
  }

  function includes(this: any, searchElement: any, fromIndex?: any): boolean {
    validateTypedArray(this, "includes");
    const len = this.length;
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
  }

  // --- Predicates / Searchers ---

  function find(this: any, predicate: any, thisArg?: any): any {
    validateTypedArray(this, "find");
    if (typeof predicate !== "function") {
      throw new TypeError(predicate + " is not a function");
    }
    const len = this.length;
    for (let k = 0; k < len; k++) {
      const kValue = this[k];
      if (predicate.call(thisArg, kValue, k, this)) {
        return kValue;
      }
    }
    return undefined;
  }

  function findIndex(this: any, predicate: any, thisArg?: any): number {
    validateTypedArray(this, "findIndex");
    if (typeof predicate !== "function") {
      throw new TypeError(predicate + " is not a function");
    }
    const len = this.length;
    for (let k = 0; k < len; k++) {
      const kValue = this[k];
      if (predicate.call(thisArg, kValue, k, this)) {
        return k;
      }
    }
    return -1;
  }

  function findLast(this: any, predicate: any, thisArg?: any): any {
    validateTypedArray(this, "findLast");
    if (typeof predicate !== "function") {
      throw new TypeError(predicate + " is not a function");
    }
    const len = this.length;
    for (let k = len - 1; k >= 0; k--) {
      const kValue = this[k];
      if (predicate.call(thisArg, kValue, k, this)) {
        return kValue;
      }
    }
    return undefined;
  }

  function findLastIndex(this: any, predicate: any, thisArg?: any): number {
    validateTypedArray(this, "findLastIndex");
    if (typeof predicate !== "function") {
      throw new TypeError(predicate + " is not a function");
    }
    const len = this.length;
    for (let k = len - 1; k >= 0; k--) {
      const kValue = this[k];
      if (predicate.call(thisArg, kValue, k, this)) {
        return k;
      }
    }
    return -1;
  }

  function some(this: any, predicate: any, thisArg?: any): boolean {
    validateTypedArray(this, "some");
    if (typeof predicate !== "function") {
      throw new TypeError(predicate + " is not a function");
    }
    const len = this.length;
    for (let k = 0; k < len; k++) {
      if (predicate.call(thisArg, this[k], k, this)) {
        return true;
      }
    }
    return false;
  }

  function every(this: any, predicate: any, thisArg?: any): boolean {
    validateTypedArray(this, "every");
    if (typeof predicate !== "function") {
      throw new TypeError(predicate + " is not a function");
    }
    const len = this.length;
    for (let k = 0; k < len; k++) {
      if (!predicate.call(thisArg, this[k], k, this)) {
        return false;
      }
    }
    return true;
  }

  // --- Reducers / Iterators ---

  function forEach(this: any, callback: any, thisArg?: any): void {
    validateTypedArray(this, "forEach");
    if (typeof callback !== "function") {
      throw new TypeError(callback + " is not a function");
    }
    const len = this.length;
    for (let k = 0; k < len; k++) {
      callback.call(thisArg, this[k], k, this);
    }
  }

  function map(this: any, callback: any, thisArg?: any): any {
    validateTypedArray(this, "map");
    if (typeof callback !== "function") {
      throw new TypeError(callback + " is not a function");
    }
    const len = this.length;
    const result = typedArraySpeciesCreate(this, len, "map");
    for (let k = 0; k < len; k++) {
      result[k] = callback.call(thisArg, this[k], k, this);
    }
    return result;
  }

  function filter(this: any, callback: any, thisArg?: any): any {
    validateTypedArray(this, "filter");
    if (typeof callback !== "function") {
      throw new TypeError(callback + " is not a function");
    }
    const len = this.length;
    const kept: any[] = [];
    for (let k = 0; k < len; k++) {
      const val = this[k];
      if (callback.call(thisArg, val, k, this)) {
        kept.push(val);
      }
    }
    const result = typedArraySpeciesCreate(this, kept.length, "filter");
    for (let n = 0; n < kept.length; n++) {
      result[n] = kept[n];
    }
    return result;
  }

  function reduce(this: any, callback: any, initialValue?: any): any {
    validateTypedArray(this, "reduce");
    if (typeof callback !== "function") {
      throw new TypeError(callback + " is not a function");
    }
    const len = this.length;
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
      accumulator = callback.call(undefined, accumulator, this[k], k, this);
      k++;
    }
    return accumulator;
  }

  function reduceRight(this: any, callback: any, initialValue?: any): any {
    validateTypedArray(this, "reduceRight");
    if (typeof callback !== "function") {
      throw new TypeError(callback + " is not a function");
    }
    const len = this.length;
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
      accumulator = callback.call(undefined, accumulator, this[k], k, this);
      k--;
    }
    return accumulator;
  }

  function join(this: any, separator?: any): string {
    validateTypedArray(this, "join");
    const len = this.length;
    const sep = separator === undefined ? "," : String(separator);
    let res = "";
    for (let k = 0; k < len; k++) {
      if (k > 0) res += sep;
      const el = this[k];
      if (el !== undefined && el !== null) {
        res += String(el);
      }
    }
    return res;
  }

  function createTypedArrayIterator(ta: any, kind: "entries" | "keys" | "values"): any {
    let index = 0;
    const iterator = {
      next() {
        if (isDetached(ta)) {
          throw new TypeError("Cannot iterate over detached ArrayBuffer");
        }
        if (index >= ta.length) {
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

  function entries(this: any): any {
    validateTypedArray(this, "entries");
    return createTypedArrayIterator(this, "entries");
  }

  function keys(this: any): any {
    validateTypedArray(this, "keys");
    return createTypedArrayIterator(this, "keys");
  }

  function values(this: any): any {
    validateTypedArray(this, "values");
    return createTypedArrayIterator(this, "values");
  }

  // --- Mutators / Copiers ---

  function slice(this: any, start: any, end: any): any {
    validateTypedArray(this, "slice");
    const len = this.length;
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
      for (let n = 0; n < count; n++, k++) {
        result[n] = this[k];
      }
    }
    return result;
  }

  function subarray(this: any, begin: any, end: any): any {
    requireTypedArray(this, "subarray");
    const buffer = this.buffer;
    const srcLength = this.length;
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
  }

  function set(this: any, source: any, offset?: any): void {
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

    const targetLength = this.length;
    const isView = ArrayBuffer.isView(source) && !(typeof DataView === "function" && source instanceof DataView);

    if (isView) {
      if (isDetached(source)) {
        throw new TypeError("Source ArrayBuffer is detached");
      }
      const srcLength = (source as any).length;
      if (targetOffset + srcLength > targetLength || targetOffset > targetLength) {
        throw new RangeError("offset is out of bounds");
      }
      if (isBigIntArray(this) !== isBigIntArray(source)) {
        throw new TypeError("Cannot mix BigInt and other types, use explicit conversions");
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
  }

  function copyWithin(this: any, target: any, start: any, end?: any): any {
    validateTypedArray(this, "copyWithin");
    const len = this.length;
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
    if (count > 0) {
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
  }

  function fill(this: any, value: any, start?: any, end?: any): any {
    validateTypedArray(this, "fill");
    const len = this.length;
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

    while (k < final) {
      this[k] = numericValue;
      k++;
    }
    return this;
  }

  function reverse(this: any): any {
    validateTypedArray(this, "reverse");
    const len = this.length;
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
  }

  function sort(this: any, compareFn?: any): any {
    validateTypedArray(this, "sort");
    if (compareFn !== undefined && typeof compareFn !== "function") {
      throw new TypeError("The comparison function must be either a function or undefined");
    }

    const len = this.length;
    if (len <= 1) return this;

    const items: any[] = [];
    for (let i = 0; i < len; i++) {
      items.push(this[i]);
    }

    const isBigInt = isBigIntArray(this);

    if (compareFn !== undefined) {
      items.sort((x, y) => {
        const res = Number(compareFn(x, y));
        if (Number.isNaN(res)) return 0;
        return res;
      });
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

    if (isDetached(this)) {
      throw new TypeError("Method %TypedArray%.prototype.sort called on detached ArrayBuffer");
    }

    for (let i = 0; i < len; i++) {
      this[i] = items[i];
    }

    return this;
  }

  // --- ES2023 change-by-copy ---

  function at(this: any, index: any): any {
    validateTypedArray(this, "at");
    const len = this.length;
    const relativeIndex = toIntegerOrInfinity(index);
    const k = relativeIndex >= 0 ? relativeIndex : len + relativeIndex;
    if (k < 0 || k >= len) {
      return undefined;
    }
    return this[k];
  }

  function toReversed(this: any): any {
    validateTypedArray(this, "toReversed");
    const len = this.length;
    const result = typedArrayCreateSameType(this, len);
    for (let i = 0; i < len; i++) {
      result[i] = this[len - 1 - i];
    }
    return result;
  }

  function toSorted(this: any, compareFn?: any): any {
    validateTypedArray(this, "toSorted");
    if (compareFn !== undefined && typeof compareFn !== "function") {
      throw new TypeError("The comparison function must be either a function or undefined");
    }

    const len = this.length;
    const items: any[] = [];
    for (let i = 0; i < len; i++) {
      items.push(this[i]);
    }

    const isBigInt = isBigIntArray(this);

    if (compareFn !== undefined) {
      items.sort((x, y) => {
        const res = Number(compareFn(x, y));
        if (Number.isNaN(res)) return 0;
        return res;
      });
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

    const result = typedArrayCreateSameType(this, len);
    for (let i = 0; i < len; i++) {
      result[i] = items[i];
    }
    return result;
  }

  function withMethod(this: any, index: any, value: any): any {
    validateTypedArray(this, "with");
    const len = this.length;
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
  }
  Object.defineProperty(withMethod, "name", { value: "with", configurable: true });

  // --- String conversions and extensions ---

  function toStringMethod(this: any): string {
    if (typeof this?.join === "function") {
      return this.join();
    }
    return Object.prototype.toString.call(this);
  }

  function toLocaleString(this: any, locales?: any, options?: any): string {
    validateTypedArray(this, "toLocaleString");
    const len = this.length;
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
  }

  function parallelMap(this: any, callback: any, thisArg?: any): any {
    requireTypedArray(this, "parallelMap");
    return this.map(callback, thisArg);
  }

  // --- Method installation ---

  function defineMethod(name: string | symbol, fn: any): void {
    Object.defineProperty(Proto, name, {
      value: fn,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  }

  // Search
  defineMethod("indexOf", indexOf);
  defineMethod("lastIndexOf", lastIndexOf);
  defineMethod("includes", includes);

  // Predicates / Searchers
  defineMethod("find", find);
  defineMethod("findIndex", findIndex);
  defineMethod("findLast", findLast);
  defineMethod("findLastIndex", findLastIndex);
  defineMethod("some", some);
  defineMethod("every", every);

  // Reducers / Iterators
  defineMethod("forEach", forEach);
  defineMethod("map", map);
  defineMethod("filter", filter);
  defineMethod("reduce", reduce);
  defineMethod("reduceRight", reduceRight);
  defineMethod("join", join);
  defineMethod("entries", entries);
  defineMethod("keys", keys);
  defineMethod("values", values);
  if (typeof Symbol !== "undefined" && Symbol.iterator) {
    defineMethod(Symbol.iterator, values);
  }

  // Mutators / Copiers
  defineMethod("slice", slice);
  defineMethod("subarray", subarray);
  defineMethod("set", set);
  defineMethod("copyWithin", copyWithin);
  defineMethod("fill", fill);
  defineMethod("reverse", reverse);
  defineMethod("sort", sort);

  // ES2023 change-by-copy
  defineMethod("at", at);
  defineMethod("toReversed", toReversed);
  defineMethod("toSorted", toSorted);
  defineMethod("with", withMethod);

  // Additional %TypedArray%.prototype methods
  defineMethod(
    "toString",
    typeof Array.prototype.toString === "function" ? Array.prototype.toString : toStringMethod
  );
  defineMethod("toLocaleString", toLocaleString);

  // kiln's own members (ADR 0068): declared in `kiln_builtin_surface::KILN_EXTENSIONS`, installed
  // only when the realm installs extensions (`extensions.ts`), and non-enumerable like the rest.
  function defineExtension(name: string, fn: any): void {
    if (kilnExtensions) {
      defineMethod(name, fn);
    }
  }
  defineExtension("parallelMap", parallelMap);
})();
