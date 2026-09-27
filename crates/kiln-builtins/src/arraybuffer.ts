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

(function installArrayBufferExtensions() {
  if (typeof ArrayBuffer === "undefined" || !ArrayBuffer.prototype) {
    return;
  }

  function defineMethod(target: any, name: string | symbol, fn: any): void {
    Object.defineProperty(target, name, {
      value: fn,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  }

  // --- ArrayBuffer.prototype extensions ---

  const Proto = ArrayBuffer.prototype as any;

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
    if (!(thisArg instanceof ArrayBuffer)) {
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

  function transfer(this: any, newByteLength?: number): ArrayBuffer {
    return doTransfer(this, newByteLength, true, "transfer");
  }

  function transferToFixedLength(this: any, newByteLength?: number): ArrayBuffer {
    return doTransfer(this, newByteLength, false, "transferToFixedLength");
  }

  function resize(this: any, newByteLength: number): void {
    if (!(this instanceof ArrayBuffer)) {
      throw new TypeError("Method ArrayBuffer.prototype.resize called on incompatible receiver");
    }
    if (isDetachedBuffer(this)) {
      throw new TypeError("Cannot resize detached ArrayBuffer");
    }
    if (!(this as any).resizable) {
      throw new TypeError("ArrayBuffer is not resizable");
    }
    const newLen = Math.trunc(Number(newByteLength));
    if (Number.isNaN(newLen) || newLen < 0 || newLen > ((this as any).maxByteLength || 0)) {
      throw new RangeError("Invalid length");
    }
    (this as any).byteLength = newLen;
  }

  defineMethod(Proto, "transfer", transfer);
  defineMethod(Proto, "transferToFixedLength", transferToFixedLength);
  defineMethod(Proto, "resize", resize);

  // --- SharedArrayBuffer.prototype extensions ---

  if (typeof SharedArrayBuffer !== "undefined" && SharedArrayBuffer.prototype) {
    const SProto = SharedArrayBuffer.prototype as any;

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

    function grow(this: any, newByteLength: number): void {
      if (!(this instanceof SharedArrayBuffer)) {
        throw new TypeError("Method SharedArrayBuffer.prototype.grow called on incompatible receiver");
      }
      if (!(this as any).growable) {
        throw new TypeError("SharedArrayBuffer is not growable");
      }
      const newLen = Math.trunc(Number(newByteLength));
      if (Number.isNaN(newLen) || newLen < 0) {
        throw new RangeError("Invalid length");
      }
      const currentLen = (this as any).byteLength || 0;
      const maxLen = (this as any).maxByteLength || currentLen;
      if (newLen < currentLen || newLen > maxLen) {
        throw new RangeError("Invalid length");
      }
      (this as any).byteLength = newLen;
    }

    function slice(this: any, begin?: number, end?: number): any {
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
      const Ctor = getSpeciesConstructor(this, SharedArrayBuffer);
      let newBuffer: any;
      try {
        newBuffer = new Ctor(newLen);
      } catch {
        newBuffer = new SharedArrayBuffer(newLen);
      }

      if (newLen > 0) {
        const src = new Uint8Array(this, first, newLen);
        const dst = new Uint8Array(newBuffer, 0, newLen);
        dst.set(src);
      }

      return newBuffer;
    }

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
