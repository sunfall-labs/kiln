// kiln-builtins: `Atomics` for the native backend (ECMAScript 25.4; ADR 0005, ADR 0063 rule 4;
// realms issue 06).
//
// Every check a program can observe -- the kind of view, whether its buffer is shared, the index,
// the conversion of each operand, in the specification's order -- is made here, and the engine's
// primitive does only the one indivisible access to the element, or the wait and wake on the
// runtime's waiter list that every realm's `Atomics.wait` and `Atomics.notify` share
// (`kiln_runtime::workers`). The interpreter's `Atomics` is its own, and it publishes no
// primitive, so none of this is installed there.
const __kilnAtomics: ((op: number, view: any, index: number, value?: any, extra?: any) => any) | undefined =
  (globalThis as any).__kilnAtomics;
delete (globalThis as any).__kilnAtomics;

(function installAtomics() {
  const primitive = __kilnAtomics;
  const G = globalThis as any;
  if (typeof primitive !== "function" || G.Atomics === null || typeof G.Atomics !== "object") {
    return;
  }
  const target = G.Atomics;
  const LOAD = 1, STORE = 2, ADD = 3, SUB = 4, AND = 5, OR = 6, XOR = 7, EXCHANGE = 8;
  const COMPARE_EXCHANGE = 9, WAIT = 10, NOTIFY = 11;
  const objectTag = Object.prototype.toString;
  const INTEGER_KINDS = ["Int8Array", "Uint8Array", "Int16Array", "Uint16Array", "Int32Array", "Uint32Array",
    "BigInt64Array", "BigUint64Array"];

  function kindOf(view: any): string {
    return objectTag.call(view).slice(8, -1);
  }

  function isBigKind(kind: string): boolean {
    return kind === "BigInt64Array" || kind === "BigUint64Array";
  }

  // `ValidateIntegerTypedArray`, and for `wait` and `notify` the narrower kinds they take.
  function validate(view: any, waitable: boolean): string {
    const kind = kindOf(view);
    if (!ArrayBuffer.isView(view) || kind === "DataView") {
      throw new TypeError(`${objectTag.call(view)} is not an integer typed array.`);
    }
    if (waitable) {
      if (kind !== "Int32Array" && kind !== "BigInt64Array") {
        throw new TypeError(`${objectTag.call(view)} is not an int32 or BigInt64 typed array.`);
      }
    } else if (INTEGER_KINDS.indexOf(kind) < 0) {
      throw new TypeError(`${objectTag.call(view)} is not an integer typed array.`);
    }
    return kind;
  }

  function toIntegerOrInfinity(value: any): number {
    const number = +value;
    if (number !== number || number === 0) return 0;
    if (number === Infinity || number === -Infinity) return number;
    return Math.trunc(number);
  }

  // `ValidateAtomicAccess`: `ToIndex`, then the bound.
  function accessIndex(view: any, index: any): number {
    const integer = index === undefined ? 0 : toIntegerOrInfinity(index);
    if (integer < 0 || integer > 9007199254740991 || integer >= view.length) {
      throw new RangeError("Invalid atomic access index");
    }
    return integer;
  }

  // The operand as the element kind converts it: `ToIntegerOrInfinity` for a number kind; a
  // BigInt kind's goes to the engine as given, whose `ToBigInt` refuses a number as the
  // specification's does.
  function operand(kind: string, value: any): any {
    return isBigKind(kind) ? value : toIntegerOrInfinity(value);
  }

  function readModifyWrite(op: number, view: any, index: any, value: any): any {
    const kind = validate(view, false);
    const at = accessIndex(view, index);
    return primitive(op, view, at, operand(kind, value));
  }

  const methods: Record<string, any> = {
    add: { add(view: any, index: any, value: any): any { return readModifyWrite(ADD, view, index, value); } }.add,
    and: { and(view: any, index: any, value: any): any { return readModifyWrite(AND, view, index, value); } }.and,
    compareExchange: { compareExchange(view: any, index: any, expected: any, replacement: any): any {
      const kind = validate(view, false);
      const at = accessIndex(view, index);
      const expectedValue = operand(kind, expected);
      return primitive(COMPARE_EXCHANGE, view, at, expectedValue, operand(kind, replacement));
    } }.compareExchange,
    exchange: { exchange(view: any, index: any, value: any): any { return readModifyWrite(EXCHANGE, view, index, value); } }.exchange,
    isLockFree: { isLockFree(size: any): boolean {
      const n = toIntegerOrInfinity(size);
      return n === 1 || n === 2 || n === 4 || n === 8;
    } }.isLockFree,
    load: { load(view: any, index: any): any {
      validate(view, false);
      return primitive(LOAD, view, accessIndex(view, index));
    } }.load,
    notify: { notify(view: any, index: any, count: any): number {
      validate(view, true);
      const at = accessIndex(view, index);
      const wanted = count === undefined ? Infinity : Math.max(toIntegerOrInfinity(count), 0);
      return primitive(NOTIFY, view, at, wanted);
    } }.notify,
    or: { or(view: any, index: any, value: any): any { return readModifyWrite(OR, view, index, value); } }.or,
    pause: { pause(iterations: any): void {
      if (iterations !== undefined && !Number.isInteger(iterations)) {
        throw new TypeError("Atomics.pause: the argument must be an integer or undefined");
      }
    } }.pause,
    store: { store(view: any, index: any, value: any): any {
      const kind = validate(view, false);
      const at = accessIndex(view, index);
      const converted = operand(kind, value);
      const stored = primitive(STORE, view, at, converted);
      if (isBigKind(kind)) return typeof value === "bigint" ? value : stored;
      return converted === 0 ? 0 : converted;
    } }.store,
    sub: { sub(view: any, index: any, value: any): any { return readModifyWrite(SUB, view, index, value); } }.sub,
    wait: { wait(view: any, index: any, value: any, timeout: any): string {
      const kind = validate(view, true);
      if (objectTag.call(view.buffer) !== "[object SharedArrayBuffer]") {
        throw new TypeError(`${objectTag.call(view)} is not a shared typed array.`);
      }
      const at = accessIndex(view, index);
      const expected = operand(kind, value);
      const limit = timeout === undefined ? Infinity : +timeout;
      return primitive(WAIT, view, at, expected, limit !== limit ? Infinity : Math.max(limit, 0));
    } }.wait,
    waitAsync: { waitAsync(view: any, index: any, value: any, timeout: any): any {
      const kind = validate(view, true);
      if (objectTag.call(view.buffer) !== "[object SharedArrayBuffer]") {
        throw new TypeError(`${objectTag.call(view)} is not a shared typed array.`);
      }
      const at = accessIndex(view, index);
      const expected = operand(kind, value);
      const limit = timeout === undefined ? Infinity : +timeout;
      const duration = limit !== limit ? Infinity : Math.max(limit, 0);
      if (primitive(LOAD, view, at) !== expected) return { async: false, value: "not-equal" };
      if (duration === 0) return { async: false, value: "timed-out" };
      // The turn loop cannot park a realm on the provider's synchronous waiter list.
      // Observe the shared element from later turns so another realm can advance it.
      // This also keeps a thread-stream worker alive while its parent flushes.
      return { async: true, value: new Promise<string>((resolve) => {
        const deadline = duration === Infinity ? Infinity : Date.now() + duration;
        const check = () => {
          if (primitive(LOAD, view, at) !== expected) { resolve("ok"); return; }
          if (Date.now() >= deadline) { resolve("timed-out"); return; }
          setTimeout(check, Math.min(10, deadline - Date.now()));
        };
        setTimeout(check, 1);
      }) };
    } }.waitAsync,
    xor: { xor(view: any, index: any, value: any): any { return readModifyWrite(XOR, view, index, value); } }.xor,
  };
  // One descriptor for every member, not a literal per call: `defineProperty` reads its fields and
  // keeps nothing, and a literal per member was the largest share of the garbage every realm's
  // bootstrap made (realms issue 08).
  const methodDescriptor: PropertyDescriptor = { value: undefined, writable: true, enumerable: false, configurable: true };
  // An index loop, not `for...of`, which makes an iterator result per member (issue 08 again).
  const names = Object.keys(methods);
  for (let i = 0; i < names.length; i++) {
    methodDescriptor.value = methods[names[i]];
    Object.defineProperty(target, names[i], methodDescriptor);
  }
})();
