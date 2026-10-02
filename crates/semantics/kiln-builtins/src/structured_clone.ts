// kiln-builtins: `structuredClone` and the structured clone every message port uses (HTML
// "structured serialize with transfer", ADR 0060 kind 7, ADR 0070 primitives A and C).
//
// One implementation for both engines. Until 2026-09-26 there were two: a JavaScript one in the
// native runtime's bootstrap and a Rust one in the interpreter that answered the *same* Map, Set
// and RegExp it was given instead of copies. Each engine now reaches engine storage through one
// declared primitive only, `__kilnArrayBufferDetach` (captured in `arraybuffer.ts`), which moves
// a buffer's backing store to a new ArrayBuffer without copying it -- the transfer half.
//
// The clone is a direct graph copy rather than serialize-then-deserialize: in one realm there is
// no wire to cross, and building the copy as the walk goes is what makes a small message cost a
// handful of allocations. Shared references and cycles are preserved through `seen`.
//
// A transfer list is validated before anything is copied, and nothing is detached until the whole
// value has been copied: a clone that throws half way (a function three levels down) leaves every
// buffer in the list attached, as node does. A transferred buffer, and a view over one, therefore
// copies as a `Pending` placeholder, and the containers that received one are patched once the
// buffers have moved.

// The plain-data fast path (ADR 0065, measured): a copy of a value made only of plain objects,
// arrays and primitives, in one engine call, or `undefined` when the value has anything else --
// nothing it reads can run program code, so answering `undefined` half way leaves nothing to undo.
const __kilnCloneData: (value: any, mode?: number) => any = (globalThis as any).__kilnCloneData;
delete (globalThis as any).__kilnCloneData;
// The shared-value predicate is an extension's (kiln:shared), lent through the engine-private
// object only when extensions are installed; without it no value is ever shared.
const __kilnSharedIs: (value: any) => boolean =
  typeof kilnPrivate.sharedIs === "function" ? kilnPrivate.sharedIs : () => false;

const __kilnStructuredClone = (function installStructuredClone() {
  const G = globalThis as any;
  const ObjectProto = Object.prototype;
  const ArrayProto = Array.prototype;
  const getPrototypeOf = Object.getPrototypeOf;
  const keysOf = Object.keys;
  const isArray = Array.isArray;
  const valuesOf = Object.values;
  const symbolsOf = Object.getOwnPropertySymbols;
  const defineProperty = Object.defineProperty;
  const objectToString = Object.prototype.toString;
  const hasOwn = Object.prototype.hasOwnProperty;
  const isView = ArrayBuffer.isView;
  const bufferSlice = ArrayBuffer.prototype.slice;
  const detachBuffer = __kilnDetachBuffer;
  const typedConstructors: any[] = [];
  const typedNames = ["Uint8Array", "Int8Array", "Uint8ClampedArray", "Int16Array", "Uint16Array", "Int32Array",
    "Uint32Array", "Float32Array", "Float64Array", "BigInt64Array", "BigUint64Array", "Float16Array"];
  // An index loop, not `for...of`, which makes an iterator result per name in every realm's
  // bootstrap (realms issue 08).
  for (let i = 0; i < typedNames.length; i++) {
    const name = typedNames[i];
    if (typeof G[name] === "function") typedConstructors.push(G[name]);
  }

  // `markAsUntransferable` (node:worker_threads). Weak, so marking an object does not keep it.
  const untransferable = new WeakSet<object>();
  // Host objects the web bootstrap teaches the clone about (MessagePort): each answers whether a
  // value is one of its kind, and the clone refuses a listed-nowhere one instead of copying its
  // fields as a plain object.
  const hostKinds: ((value: any) => string | undefined)[] = [];

  function dataCloneError(message: string): Error {
    const DOMExc = G.DOMException;
    if (typeof DOMExc === "function") return new DOMExc(message, "DataCloneError");
    const error: any = new Error(message);
    error.name = "DataCloneError";
    error.code = 25;
    return error;
  }

  function notCloneable(value: any): Error {
    if (typeof value === "symbol") return dataCloneError(String(value) + " could not be cloned.");
    if (typeof value === "function") return dataCloneError(String(value).split("\n")[0] + " could not be cloned.");
    let name = "Object";
    try {
      const ctor = value.constructor;
      if (ctor && typeof ctor.name === "string" && ctor.name !== "") name = ctor.name;
    } catch {}
    return dataCloneError("#<" + name + "> could not be cloned.");
  }

  // A transferred object's copy, which exists only once the whole value has been copied.
  class Pending {
    value: any = undefined;
    make: () => any;
    constructor(make: () => any) {
      this.make = make;
    }
  }

  // What one clone with a non-empty transfer list carries: the placeholders in the order they
  // must be made (buffers before the views over them), and the containers to patch.
  interface TransferState {
    pending: Pending[];
    fixups: any[];
  }

  function isPending(state: TransferState | null, value: any): boolean {
    return state !== null && value instanceof Pending;
  }

  function cloneValue(value: any, seen: Map<any, any>, state: TransferState | null): any {
    if (typeof value !== "object" || value === null) {
      if (typeof value === "function" || typeof value === "symbol") throw notCloneable(value);
      return value;
    }
    if (__kilnSharedIs(value)) return value;
    const found = seen.get(value);
    if (found !== undefined) return found;
    if (isArray(value)) {
      return getPrototypeOf(value) === ArrayProto ? cloneArray(value, seen, state) : cloneArrayLike(value, seen, state);
    }
    // The tag first: it is what tells an ordinary object from the built-ins below, and the
    // native backend refuses `Object.getPrototypeOf` and `instanceof` on a few built-ins whose
    // instances it does not model (a Promise), which must reach the refusal as a DataCloneError.
    const tag = objectToString.call(value);
    if (tag === "[object Object]" && getPrototypeOf(value) === ObjectProto) return clonePlain(value, seen, state);
    return cloneOther(value, tag, seen, state);
  }

  // The own enumerable string-keyed properties, read with [[Get]] as the specification does.
  //
  // `{ ...value }` is CopyDataProperties: every own enumerable property read with [[Get]] and
  // defined on a fresh ordinary object -- a `__proto__` key included, as an own property. That
  // is the whole copy for the primitive-valued properties, made in one engine operation instead
  // of a dynamic-key read and write per property, which were most of what a small message cost.
  // The walk below then replaces the object-valued ones with their clones and refuses a function
  // or a symbol. The one thing spread copies that a structured clone does not is an enumerable
  // symbol-keyed property; those are removed again.
  function clonePlain(value: any, seen: Map<any, any>, state: TransferState | null): any {
    // An own `__proto__` (from `JSON.parse`, say) is the one key the engines' spread does not
    // copy as an ordinary own property, so that object takes the key-by-key copy.
    if (hasOwn.call(value, "__proto__")) return clonePlainByKey(value, seen, state);
    const out = { ...value };
    seen.set(value, out);
    const values = valuesOf(out);
    let keys: string[] | null = null;
    for (let i = 0; i < values.length; i++) {
      const item = values[i];
      if (typeof item !== "object" || item === null) {
        if (typeof item === "function" || typeof item === "symbol") throw notCloneable(item);
        continue;
      }
      if (keys === null) keys = keysOf(out);
      const key = keys[i];
      const copy = cloneValue(item, seen, state);
      if (isPending(state, copy)) (state as TransferState).fixups.push(out, key);
      out[key] = copy;
    }
    const symbols = symbolsOf(out);
    for (let i = 0; i < symbols.length; i++) delete out[symbols[i]];
    return out;
  }

  function clonePlainByKey(value: any, seen: Map<any, any>, state: TransferState | null): any {
    const out: any = {};
    seen.set(value, out);
    const keys = keysOf(value);
    for (let i = 0; i < keys.length; i++) {
      const key = keys[i];
      const item = value[key];
      let copy: any = item;
      if (typeof item === "object" && item !== null) {
        copy = cloneValue(item, seen, state);
        if (isPending(state, copy)) (state as TransferState).fixups.push(out, key);
      } else if (typeof item === "function" || typeof item === "symbol") {
        throw notCloneable(item);
      }
      defineProperty(out, key, { value: copy, writable: true, enumerable: true, configurable: true });
    }
    return out;
  }

  // Visit own indices only. `slice` reads inherited numeric getters and turns their answers
  // into own elements, while a structured clone leaves an absent index absent.
  function cloneArray(value: any[], seen: Map<any, any>, state: TransferState | null): any[] {
    const out = new Array(value.length);
    seen.set(value, out);
    for (let i = 0; i < out.length; i++) {
      if (!hasOwn.call(value, i)) continue;
      const item = value[i];
      if (typeof item !== "object" || item === null) {
        if (typeof item === "function" || typeof item === "symbol") throw notCloneable(item);
        defineProperty(out, i, { value: item, writable: true, enumerable: true, configurable: true });
        continue;
      }
      const copy = cloneValue(item, seen, state);
      if (isPending(state, copy)) (state as TransferState).fixups.push(out, i);
      defineProperty(out, i, { value: copy, writable: true, enumerable: true, configurable: true });
    }
    return out;
  }

  // A subclass also becomes a plain array and copies only its own elements.
  function cloneArrayLike(value: any[], seen: Map<any, any>, state: TransferState | null): any[] {
    const length = value.length;
    const out = new Array(length);
    seen.set(value, out);
    for (let i = 0; i < length; i++) {
      if (!hasOwn.call(value, i)) continue;
      const item = value[i];
      if (typeof item !== "object" || item === null) {
        if (typeof item === "function" || typeof item === "symbol") throw notCloneable(item);
        defineProperty(out, i, { value: item, writable: true, enumerable: true, configurable: true });
        continue;
      }
      const copy = cloneValue(item, seen, state);
      if (isPending(state, copy)) (state as TransferState).fixups.push(out, i);
      defineProperty(out, i, { value: copy, writable: true, enumerable: true, configurable: true });
    }
    return out;
  }

  function cloneBuffer(value: ArrayBuffer, seen: Map<any, any>): ArrayBuffer {
    if ((value as any).detached) throw dataCloneError("An ArrayBuffer is detached and could not be cloned.");
    const out = bufferSlice.call(value, 0);
    seen.set(value, out);
    return out;
  }

  function typedConstructorOf(value: any): any {
    for (let i = 0; i < typedConstructors.length; i++) {
      if (value instanceof typedConstructors[i]) return typedConstructors[i];
    }
    return undefined;
  }

  // A view copies its whole buffer, not only its window, and shares that copy with every other
  // view of the same buffer in the value: `r.a.buffer === r.b` when `a` was a view of `b`.
  function cloneView(value: any, seen: Map<any, any>, state: TransferState | null): any {
    const Ctor = value instanceof DataView ? DataView : typedConstructorOf(value);
    if (Ctor === undefined) throw notCloneable(value);
    const offset = value.byteOffset;
    const length = Ctor === DataView ? value.byteLength : value.length;
    const buffer = cloneValue(value.buffer, seen, state);
    if (isPending(state, buffer)) {
      const view = new Pending(() => new Ctor((buffer as Pending).value, offset, length));
      (state as TransferState).pending.push(view);
      seen.set(value, view);
      return view;
    }
    const out = new Ctor(buffer, offset, length);
    seen.set(value, out);
    return out;
  }

  function cloneError(value: any, seen: Map<any, any>, state: TransferState | null): any {
    const name = value.name;
    const Ctor = name === "EvalError" ? EvalError : name === "RangeError" ? RangeError
      : name === "ReferenceError" ? ReferenceError : name === "SyntaxError" ? SyntaxError
      : name === "TypeError" ? TypeError : name === "URIError" ? URIError : Error;
    const out: any = new Ctor(value.message);
    seen.set(value, out);
    if ("stack" in value) out.stack = value.stack;
    if ("cause" in value) out.cause = cloneValue(value.cause, seen, state);
    return out;
  }

  function cloneOther(value: any, tag: string, seen: Map<any, any>, state: TransferState | null): any {
    if (tag === "[object WeakMap]" || tag === "[object WeakSet]" || tag === "[object Promise]" ||
        tag === "[object WeakRef]" || tag === "[object FinalizationRegistry]") {
      throw notCloneable(value);
    }
    // Shared bytes are shared, not copied: in one realm the same buffer is the same memory.
    if (tag === "[object SharedArrayBuffer]") {
      seen.set(value, value);
      return value;
    }
    let out: any;
    if (value instanceof ArrayBuffer) return cloneBuffer(value, seen);
    if (isView(value)) return cloneView(value, seen, state);
    if (value instanceof Date) { out = new Date(value.getTime()); seen.set(value, out); return out; }
    if (value instanceof RegExp) { out = new RegExp(value.source, value.flags); seen.set(value, out); return out; }
    if (value instanceof Boolean || value instanceof Number || value instanceof String) {
      out = Object(value.valueOf());
      seen.set(value, out);
      return out;
    }
    if (typeof BigInt === "function" && value instanceof (BigInt as any)) {
      out = Object(value.valueOf());
      seen.set(value, out);
      return out;
    }
    if (value instanceof Error) return cloneError(value, seen, state);
    if (value instanceof Map) {
      out = new Map();
      seen.set(value, out);
      let patch = false;
      for (const [k, v] of value) {
        const key = cloneValue(k, seen, state);
        const item = cloneValue(v, seen, state);
        if (isPending(state, key) || isPending(state, item)) patch = true;
        out.set(key, item);
      }
      if (patch) (state as TransferState).fixups.push(out, undefined);
      return out;
    }
    if (value instanceof Set) {
      out = new Set();
      seen.set(value, out);
      let patch = false;
      for (const v of value) {
        const item = cloneValue(v, seen, state);
        if (isPending(state, item)) patch = true;
        out.add(item);
      }
      if (patch) (state as TransferState).fixups.push(out, undefined);
      return out;
    }
    for (let i = 0; i < hostKinds.length; i++) {
      const refusal = hostKinds[i](value);
      if (refusal !== undefined) throw dataCloneError(refusal);
    }
    // Any other object -- a class instance, a null-prototype record -- copies as a plain object
    // of its own enumerable properties, as V8's serializer does.
    return clonePlain(value, seen, state);
  }

  // The placeholders are made in order (every buffer before any view over it), then each
  // container that received one is patched. A Map or Set is rebuilt, so its order is kept.
  function resolve(value: any): any {
    return value instanceof Pending ? value.value : value;
  }

  function commit(state: TransferState): void {
    for (const pending of state.pending) pending.value = pending.make();
    const fixups = state.fixups;
    for (let i = 0; i < fixups.length; i += 2) {
      const container = fixups[i];
      const key = fixups[i + 1];
      if (container instanceof Map) {
        const entries = Array.from(container);
        container.clear();
        for (const [k, v] of entries) container.set(resolve(k), resolve(v));
      } else if (container instanceof Set) {
        const items = Array.from(container);
        container.clear();
        for (const v of items) container.add(resolve(v));
      } else {
        container[key] = resolve(container[key]);
      }
    }
  }

  // `transfer` has already been read into an array; validate it, then copy `value` with every
  // listed object standing for itself. `host` makes the copy of a listed non-buffer object (a
  // MessagePort) or answers undefined when it is not one it can transfer.
  function serialize(value: any, transfer: any[] | undefined, host?: (item: any) => (() => any) | undefined): any {
    if (transfer === undefined || transfer.length === 0) {
      if (typeof value === "object" && value !== null) {
        if (__kilnSharedIs(value)) return value;
        const copy = __kilnCloneData(value);
        if (copy !== undefined) return copy;
      }
      return cloneValue(value, new Map(), null);
    }
    const seen = new Map<any, any>();
    const state: TransferState = { pending: [], fixups: [] };
    for (let i = 0; i < transfer.length; i++) {
      const item = transfer[i];
      if (__kilnSharedIs(item)) throw dataCloneError("Shared values cannot be transferred.");
      if (item instanceof ArrayBuffer) {
        if (seen.has(item)) throw dataCloneError("Transfer list contains duplicate ArrayBuffer");
        if ((item as any).detached || untransferable.has(item)) {
          throw dataCloneError("Cannot transfer object of unsupported type.");
        }
        const buffer = item;
        const pending = new Pending(() => detachBuffer(buffer, buffer.byteLength, true));
        state.pending.push(pending);
        seen.set(item, pending);
        continue;
      }
      const make = host !== undefined && item !== null && typeof item === "object" ? host(item) : undefined;
      if (make === undefined) throw dataCloneError("Found invalid value in transferList.");
      if (seen.has(item)) throw dataCloneError("Transfer list contains duplicate");
      const pending = new Pending(make);
      state.pending.push(pending);
      seen.set(item, pending);
    }
    const out = cloneValue(value, seen, state);
    commit(state);
    return resolve(out);
  }

  // The `transfer` member of a StructuredSerializeOptions dictionary, as a sequence.
  function transferOption(options: any, what: string): any[] | undefined {
    if (options === undefined || options === null) return undefined;
    if (typeof options !== "object" && typeof options !== "function") {
      const error: any = new TypeError(what === "structuredClone"
        ? "Failed to execute 'structuredClone': Options cannot be converted to a dictionary"
        : "The \"options\" argument must be of type object.");
      error.code = "ERR_INVALID_ARG_TYPE";
      throw error;
    }
    const list = options.transfer;
    if (list === undefined) return undefined;
    return sequence(list, what === "structuredClone"
      ? "Failed to execute 'structuredClone': transfer in Options cannot be converted to sequence."
      : "Optional options.transfer argument must be an iterable");
  }

  function sequence(list: any, message: string): any[] {
    if (Array.isArray(list)) return list;
    if (list === null || (typeof list !== "object" && typeof list !== "function") ||
        typeof list[Symbol.iterator] !== "function") {
      const error: any = new TypeError(message);
      error.code = "ERR_INVALID_ARG_TYPE";
      throw error;
    }
    return Array.from(list);
  }

  function structuredClone(value: any, options: any): any {
    // `arguments` only when it can matter: materialising it on every call cost more than
    // copying a small object.
    if (value === undefined && arguments.length === 0) {
      const error: any = new TypeError("The \"value\" argument must be specified");
      error.code = "ERR_MISSING_ARGS";
      throw error;
    }
    return serialize(value, transferOption(options, "structuredClone"));
  }

  const internal = {
    serialize,
    isShared(value: any): boolean { return __kilnSharedIs(value); },
    transferOption,
    sequence,
    dataCloneError,
    registerHostKind(check: (value: any) => string | undefined): void { hostKinds.push(check); },
    markAsUntransferable(value: any): void {
      if (value !== null && (typeof value === "object" || typeof value === "function")) untransferable.add(value);
    },
    isMarkedAsUntransferable(value: any): boolean {
      return value !== null && (typeof value === "object" || typeof value === "function") && untransferable.has(value);
    },
  };
  // `node:worker_threads` is a module, not part of this script, and reaches the marks here
  // through the engine-private object, never through a property of `structuredClone`.
  kilnPrivate.structuredClone = internal;
  defineProperty(G, "structuredClone", { value: structuredClone, writable: true, enumerable: true, configurable: true });
  return internal;
})();
