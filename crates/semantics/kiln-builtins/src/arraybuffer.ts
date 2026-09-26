// kiln-builtins: `ArrayBuffer.prototype` and `SharedArrayBuffer.prototype` extensions
// ADR 0005, ADR 0063 rule 4
//
// Shared by both interpreter and native execution.

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
    try {
      const desc = Object.getOwnPropertyDescriptor(buf, "detached");
      if (desc && desc.value !== undefined) return Boolean(desc.value);
    } catch (_) {}
    return Boolean((buf as any).detached);
  }

  function doTransfer(thisArg: any, newByteLength: any, fixedLength: boolean, methodName: string): ArrayBuffer {
    if (!(thisArg instanceof ArrayBuffer)) {
      throw new TypeError(`Method ArrayBuffer.prototype.${methodName} called on incompatible receiver`);
    }
    if (isDetachedBuffer(thisArg) || (thisArg as any).detached) {
      throw new TypeError("Cannot transfer detached ArrayBuffer");
    }
    const oldLen = thisArg.byteLength;
    const rawLen = newByteLength === undefined ? oldLen : Math.trunc(Number(newByteLength));
    if (rawLen < 0) {
      throw new RangeError("Invalid length");
    }
    const newLen = Number.isNaN(rawLen) ? 0 : rawLen;

    let newBuffer: ArrayBuffer;
    if (!fixedLength && (thisArg as any).resizable && (thisArg as any).maxByteLength !== undefined) {
      if (newLen > (thisArg as any).maxByteLength) {
        throw new RangeError("Invalid length");
      }
      try {
        newBuffer = new (ArrayBuffer as any)(newLen, { maxByteLength: (thisArg as any).maxByteLength });
        (newBuffer as any).resizable = true;
        (newBuffer as any).maxByteLength = (thisArg as any).maxByteLength;
      } catch {
        newBuffer = new ArrayBuffer(newLen);
      }
    } else {
      newBuffer = new ArrayBuffer(newLen);
    }

    const copyLen = Math.min(oldLen, newLen);
    if (copyLen > 0) {
      const src = new Uint8Array(thisArg, 0, copyLen);
      const dst = new Uint8Array(newBuffer, 0, copyLen);
      dst.set(src);
    }

    (thisArg as any).detached = true;
    try {
      Object.defineProperty(thisArg, "detached", {
        value: true,
        writable: true,
        configurable: true,
      });
    } catch (_) {}
    try {
      Object.defineProperty(thisArg, "byteLength", {
        value: 0,
        configurable: true,
      });
    } catch (_) {}

    return newBuffer;
  }

  function transfer(this: any, newByteLength?: number): ArrayBuffer {
    return doTransfer(this, newByteLength, false, "transfer");
  }

  function transferToFixedLength(this: any, newByteLength?: number): ArrayBuffer {
    return doTransfer(this, newByteLength, true, "transferToFixedLength");
  }

  function resize(this: any, newByteLength: number): void {
    if (!(this instanceof ArrayBuffer)) {
      throw new TypeError("Method ArrayBuffer.prototype.resize called on incompatible receiver");
    }
    if (isDetachedBuffer(this) || (this as any).detached) {
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
