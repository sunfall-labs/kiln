// kiln-builtins: `ArrayBuffer.prototype` and `SharedArrayBuffer.prototype` extensions
// ADR 0005, ADR 0063 rule 4
//
// Shared by both interpreter and native execution.

// The one primitive a transfer needs (ADR 0065, representation): detach `buffer` and answer a new
// ArrayBuffer of `length` bytes that owns the old backing store -- moved, not copied, grown with
// zeros or cut short -- resizable up to the old ceiling when `preserveResizability` is set. Each
// engine implements only that move; `transfer`, `transferToFixedLength` and every transfer list
// (`structured_clone.ts`) call it after making their own checks.
const __kilnDetachBuffer: (buffer: ArrayBuffer, length: number, preserveResizability: boolean) => ArrayBuffer =
  (globalThis as any).__kilnArrayBufferDetach;
delete (globalThis as any).__kilnArrayBufferDetach;
// The ordinary property read knows the ArrayBuffer internal slot, but intrinsic inspection also
// needs the actual getter function (for example, call-bound's lookup of
// `ArrayBuffer.prototype.byteLength`). Keep its slot read behind the same private engine seam.
const __kilnArrayBufferByteLength: (buffer: any, shared?: boolean) => number =
  (globalThis as any).__kilnArrayBufferByteLength;
delete (globalThis as any).__kilnArrayBufferByteLength;

// The primitive under `resize` and `grow` (ADR 0065, representation): move a resizable buffer's
// length, or grow a growable shared one, in place. Every check a program can observe is made
// above it. The native backend has no resizable buffers -- `resizable` and `growable` are never
// true there, so both methods refuse before reaching it -- and publishes none.
const __kilnResizeBuffer: ((buffer: any, length: number) => void) | undefined =
  (globalThis as any).__kilnArrayBufferResize;
delete (globalThis as any).__kilnArrayBufferResize;

// The native backend's `SharedArrayBuffer` store (ADR 0065, representation; realms issue 06):
// `(1, length, maxByteLength?)` makes a buffer over a new store every realm it is sent to shares,
// `(2, buffer, length)` grows a growable one (false when it cannot), `(6, constructor)` names
// the function `new SharedArrayBuffer(...)` runs. The interpreter builds its shared buffers
// itself and publishes none.
const __kilnSharedBuffer: ((op: number, a?: any, b?: any) => any) | undefined =
  (globalThis as any).__kilnSharedBuffer;
delete (globalThis as any).__kilnSharedBuffer;

(function installArrayBufferExtensions() {
  if (typeof ArrayBuffer === "undefined" || !ArrayBuffer.prototype) {
    return;
  }

  // Both constructors expose the standard inherited-species accessor as an own accessor.
  // Installing it through the ordinary TS surface gives ownKeys and descriptor inspection the
  // same authority as reads, including computed `Symbol.species` access.
  if (typeof Symbol !== "undefined" && Symbol.species) {
    const speciesDescriptor = Object.getOwnPropertyDescriptor({
      get [Symbol.species]() { return this; },
    }, Symbol.species)!;
    speciesDescriptor.enumerable = false;
    Object.defineProperty(ArrayBuffer, Symbol.species, speciesDescriptor);
    if (typeof SharedArrayBuffer !== "undefined") {
      Object.defineProperty(SharedArrayBuffer, Symbol.species, speciesDescriptor);
    }
  }

  // Built-in methods are not constructors, so each is a method definition (`{ m() {} }.m`)
  // rather than a function declaration, which would have [[Construct]].
  // One descriptor for every member, not a literal per call: `defineProperty` reads its fields and
  // keeps nothing, and a literal per member was the largest share of the garbage every realm's
  // bootstrap made (realms issue 08).
  const methodDescriptor: PropertyDescriptor = { value: undefined, writable: true, enumerable: false, configurable: true };
  function defineMethod(target: any, name: string | symbol, fn: any): void {
    methodDescriptor.value = fn;
    Object.defineProperty(target, name, methodDescriptor);
    methodDescriptor.value = undefined;
  }

  // --- ArrayBuffer.prototype extensions ---

  const Proto = ArrayBuffer.prototype as any;

  // Preserve the accessor as a callable TypeScript-owned function. A direct read on a valid
  // buffer uses the engine's internal-slot primitive, so subclasses and detached buffers retain
  // their brand semantics without consulting an overridable property.
  const byteLengthGetter = function (this: any): number {
    return __kilnArrayBufferByteLength(this);
  };
  Object.defineProperty(byteLengthGetter, "name", { value: "get byteLength", configurable: true });
  Object.defineProperty(Proto, "byteLength", {
    get: byteLengthGetter,
    set: undefined,
    enumerable: false,
    configurable: true,
  });

  function isDetachedBuffer(buf: any): boolean {
    return buf.detached === true;
  }

  function receiverName(value: any): string {
    if (value === null) return "null";
    if (typeof value !== "object" && typeof value !== "function") return String(value);
    const ctor = value.constructor;
    return "#<" + (ctor && typeof ctor.name === "string" && ctor.name !== "" ? ctor.name : "Object") + ">";
  }

  // `ArrayBufferCopyAndDetach`: every check a caller could observe is made here, above the
  // primitive, and the primitive only moves the backing store. So the bytes are handed over,
  // never copied -- a transfer of 16 MB costs what a transfer of 1 KB does -- and every view
  // over the source sees it empty.
  function doTransfer(thisArg: any, newByteLength: any, preserveResizability: boolean, methodName: string): ArrayBuffer {
    // An ArrayBuffer of any realm -- `instanceof` would refuse one from another realm, which is
    // what `$262.detachArrayBuffer` hands over -- and never a SharedArrayBuffer: `detached` is an
    // ArrayBuffer accessor, and reading it on anything that is not one throws or misses.
    if (thisArg === null || typeof thisArg !== "object" || typeof thisArg.detached !== "boolean") {
      throw new TypeError(`Method ArrayBuffer.prototype.${methodName} called on incompatible receiver ${receiverName(thisArg)}`);
    }
    let newLen: number;
    if (newByteLength === undefined) {
      newLen = thisArg.byteLength;
    } else {
      const number = Math.trunc(Number(newByteLength));
      newLen = Number.isNaN(number) ? 0 : number;
      if (newLen < 0 || newLen > 9007199254740991) throw new RangeError("Invalid array buffer length");
    }
    if (isDetachedBuffer(thisArg)) {
      throw new TypeError(`Cannot perform ArrayBuffer.prototype.${methodName} on a detached ArrayBuffer`);
    }
    const keepCeiling = preserveResizability && (thisArg as any).resizable === true;
    if (keepCeiling && newLen > (thisArg as any).maxByteLength) {
      throw new RangeError("Invalid array buffer length");
    }
    return __kilnDetachBuffer(thisArg, newLen, keepCeiling);
  }

  // `newLength` is optional in the specification, so both report `length` 0: a parameter with a
  // default is not counted, where a TypeScript optional one is.
  const transfer = { transfer(this: any, newLength: any = undefined): ArrayBuffer {
    return doTransfer(this, newLength, true, "transfer");
  } }.transfer;

  const transferToFixedLength = { transferToFixedLength(this: any, newLength: any = undefined): ArrayBuffer {
    return doTransfer(this, newLength, false, "transferToFixedLength");
  } }.transferToFixedLength;

  // `ToIndex`: the conversion every length argument takes, which may run the program's own
  // `valueOf` -- and that may detach or resize the buffer, which is why each caller checks the
  // buffer's state only after it.
  function toIndex(value: any): number {
    if (value === undefined) return 0;
    const number = Number(value);
    const integer = Number.isNaN(number) ? 0 : Math.trunc(number);
    if (integer < 0 || integer > 9007199254740991) throw new RangeError("Invalid array buffer length");
    return integer;
  }

  // `ArrayBuffer.prototype.resize`: only a resizable, non-shared buffer has a length to move.
  const resize = { resize(this: any, newLength: any): void {
    if (this === null || typeof this !== "object" || this.resizable !== true) {
      throw new TypeError("Method ArrayBuffer.prototype.resize called on incompatible receiver " + receiverName(this));
    }
    const newByteLength = toIndex(newLength);
    if (isDetachedBuffer(this)) {
      throw new TypeError("Cannot perform ArrayBuffer.prototype.resize on a detached ArrayBuffer");
    }
    if (newByteLength > this.maxByteLength) {
      throw new RangeError("ArrayBuffer.prototype.resize: Invalid length parameter");
    }
    (__kilnResizeBuffer as (buffer: any, length: number) => void)(this, newByteLength);
  } }.resize;

  defineMethod(Proto, "resize", resize);
  defineMethod(Proto, "transfer", transfer);
  defineMethod(Proto, "transferToFixedLength", transferToFixedLength);

  // `new SharedArrayBuffer(length, options)`, where the engine asks the bootstrap for it: every
  // check, then a store of `length` zeroed bytes that can grow to `options.maxByteLength`.
  if (typeof __kilnSharedBuffer === "function") {
    const makeShared = __kilnSharedBuffer;
    const construct = function SharedArrayBuffer(length: any, options: any): any {
      const byteLength = toIndex(length);
      let maxByteLength: number | undefined;
      if (options !== null && typeof options === "object") {
        const requested = options.maxByteLength;
        if (requested !== undefined) {
          maxByteLength = toIndex(requested);
          if (byteLength > maxByteLength) throw new RangeError("Invalid array buffer max length");
        }
      }
      return makeShared(1, byteLength, maxByteLength);
    };
    makeShared(6, construct);
  }

  // --- SharedArrayBuffer.prototype extensions ---

  if (typeof SharedArrayBuffer !== "undefined" && SharedArrayBuffer.prototype) {
    const SProto = SharedArrayBuffer.prototype as any;
    const sharedByteLengthGetter = function (this: any): number {
      return __kilnArrayBufferByteLength(this, true);
    };
    Object.defineProperty(sharedByteLengthGetter, "name", { value: "get byteLength", configurable: true });
    Object.defineProperty(SProto, "byteLength", {
      get: sharedByteLengthGetter,
      set: undefined,
      enumerable: false,
      configurable: true,
    });

    function toIntegerOrInfinity(val: any): number {
      const n = Number(val);
      if (Number.isNaN(n) || n === 0) return 0;
      if (!Number.isFinite(n)) return n;
      return Math.trunc(n);
    }

    function getSpeciesConstructor(obj: any, defaultConstructor: any): any {
      const ctor = obj.constructor;
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

    // `SharedArrayBuffer.prototype.grow`: a growable shared buffer's length only moves up.
    const grow = { grow(this: any, newLength: any): void {
      if (this === null || typeof this !== "object" || this.growable !== true) {
        throw new TypeError("Method SharedArrayBuffer.prototype.grow called on incompatible receiver " + receiverName(this));
      }
      const newByteLength = toIndex(newLength);
      if (newByteLength < this.byteLength || newByteLength > this.maxByteLength) {
        throw new RangeError("SharedArrayBuffer.prototype.grow: Invalid length parameter");
      }
      if (typeof __kilnResizeBuffer === "function") {
        __kilnResizeBuffer(this, newByteLength);
      } else if (typeof __kilnSharedBuffer !== "function" || !__kilnSharedBuffer(2, this, newByteLength)) {
        throw new RangeError("SharedArrayBuffer.prototype.grow: Invalid length parameter");
      }
    } }.grow;

    const slice = { slice(this: any, begin?: number, end?: number): any {
      if (!(this instanceof SharedArrayBuffer)) {
        throw new TypeError("Method SharedArrayBuffer.prototype.slice called on incompatible receiver");
      }
      const len = (this as any).byteLength || 0;
      const relativeStart = begin === undefined ? 0 : toIntegerOrInfinity(begin);
      let first: number;
      if (relativeStart < 0) {
        first = Math.max(len + relativeStart, 0);
      } else {
        first = Math.min(relativeStart, len);
      }

      const relativeEnd = end === undefined ? len : toIntegerOrInfinity(end);
      let final: number;
      if (relativeEnd < 0) {
        final = Math.max(len + relativeEnd, 0);
      } else {
        final = Math.min(relativeEnd, len);
      }

      const newLen = Math.max(final - first, 0);
      // `SpeciesConstructor`, then the three checks on what it built: a shared buffer, not this
      // one, and at least as long as asked -- each a TypeError, in that order.
      const Ctor = getSpeciesConstructor(this, SharedArrayBuffer);
      const newBuffer: any = new Ctor(newLen);
      if (newBuffer === null || typeof newBuffer !== "object" || typeof newBuffer.growable !== "boolean") {
        throw new TypeError("Species constructor did not return a SharedArrayBuffer");
      }
      if (newBuffer === this) {
        throw new TypeError("SharedArrayBuffer subclass returned this from species constructor");
      }
      if (newBuffer.byteLength < newLen) {
        throw new TypeError("Species constructor returned a SharedArrayBuffer that is too small");
      }

      if (newLen > 0) {
        const src = new Uint8Array(this, first, newLen);
        const dst = new Uint8Array(newBuffer, 0, newLen);
        dst.set(src);
      }

      return newBuffer;
    } }.slice;

    defineMethod(SProto, "grow", grow);
    defineMethod(SProto, "slice", slice);

    if (typeof Symbol !== "undefined" && Symbol.toStringTag) {
      Object.defineProperty(SProto, Symbol.toStringTag, {
        value: "SharedArrayBuffer",
        configurable: true,
      });
    }
  }
})();
