// kiln-builtins: `Buffer.prototype`, shared by both engines (ADR 0005, ADR 0063 rule 4).
//
// Each engine builds a Buffer as a branded `Uint8Array` -- `is_buffer` natively, the same field on
// the interpreter's view -- and answers `from`, `alloc`, `concat`, `byteLength`, `isBuffer` and a
// `toString` that decodes an encoding. It stops there: until 2026-09-19 `typeof Buffer.prototype`
// was `"undefined"` in both, and `b.equals(c)` died with "Uint8Array.equals is not a function".
//
// Everything below is that prototype, over three things the engines already have: the branded
// view's bytes, `toString(encoding)`, and `Buffer.from(arrayBuffer, offset, length)`, which
// brands a window over existing bytes so `subarray` can share memory the way node's does. A
// branded view reads its members from `Buffer.prototype` before `Uint8Array.prototype`; that
// lookup is the engine's, and the object it consults is the one assigned here.
//
// Encodings other than UTF-8 are decoded and encoded in TypeScript on purpose: hex, base64,
// latin1 and UTF-16LE are each a dozen lines, and a second implementation of each in C and in
// Rust is how the two engines came to answer `Buffer.from("ff", "hex")` as the two bytes of
// the text "ff".

(function installBufferPrototype() {
  const G = globalThis as any;
  const B = G.Buffer;
  const rawToString: (this: Uint8Array, encoding?: string) => string = G.__kilnBufferToString;
  if (typeof B !== "function" || typeof rawToString !== "function") return;
  // Captured, not called: this bootstrap runs in every Node-mode program, and a call into
  // Buffer here would be a call into a group the program may not have retained.
  delete G.__kilnBufferToString;
  const rawFrom: (value: any, offset?: number, length?: number) => Uint8Array = B.from;
  const rawAlloc: (size: number, fill?: any) => Uint8Array = B.alloc;
  const rawIsBuffer: (value: any) => boolean = B.isBuffer;
  const rawByteLength: (value: any) => number = B.byteLength;
  const U8 = Uint8Array;
  const F64 = new Float64Array(1);
  const F32 = new Float32Array(1);
  const F64_BYTES = new U8(F64.buffer);
  const F32_BYTES = new U8(F32.buffer);
  F64[0] = 1;
  const LITTLE_ENDIAN = F64_BYTES[7] === 0x3f;

  function normalizeEncoding(encoding: any): string {
    if (encoding === undefined || encoding === null) return "utf8";
    const lower = String(encoding).toLowerCase();
    switch (lower) {
      case "utf8":
      case "utf-8":
        return "utf8";
      case "ucs2":
      case "ucs-2":
      case "utf16le":
      case "utf-16le":
        return "utf16le";
      case "latin1":
      case "binary":
        return "latin1";
      case "ascii":
      case "hex":
      case "base64":
      case "base64url":
        return lower;
      default:
        return "";
    }
  }

  function checkEncoding(encoding: any): string {
    const normalized = normalizeEncoding(encoding);
    if (normalized === "") {
      const error: any = new TypeError(`Unknown encoding: ${encoding}`);
      error.code = "ERR_UNKNOWN_ENCODING";
      throw error;
    }
    return normalized;
  }

  function wrap(bytes: ArrayBuffer, offset: number, length: number): Uint8Array {
    return rawFrom(bytes, offset, length);
  }

  function fromBytes(bytes: number[]): Uint8Array {
    const out = rawAlloc(bytes.length);
    for (let i = 0; i < bytes.length; i++) out[i] = bytes[i];
    return out;
  }

  const BASE64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  const BASE64_LOOKUP: number[] = [];
  for (let i = 0; i < 64; i++) BASE64_LOOKUP[BASE64.charCodeAt(i)] = i;
  BASE64_LOOKUP["-".charCodeAt(0)] = 62;
  BASE64_LOOKUP["_".charCodeAt(0)] = 63;

  function decodeBase64(text: string): number[] {
    const out: number[] = [];
    let bits = 0;
    let have = 0;
    for (let i = 0; i < text.length; i++) {
      const code = text.charCodeAt(i);
      if (code === 61 /* = */) break;
      const value = BASE64_LOOKUP[code];
      if (value === undefined) continue; // node skips whitespace and anything it cannot read
      bits = (bits << 6) | value;
      have += 6;
      if (have >= 8) {
        have -= 8;
        out.push((bits >> have) & 0xff);
      }
    }
    return out;
  }

  function decodeHex(text: string): number[] {
    const out: number[] = [];
    for (let i = 0; i + 1 < text.length; i += 2) {
      const byte = parseInt(text.substr(i, 2), 16);
      if (Number.isNaN(byte)) break; // node stops at the first bad pair
      out.push(byte);
    }
    return out;
  }

  function encodeString(text: string, encoding: string): Uint8Array {
    switch (encoding) {
      case "utf8":
        return rawFrom(text);
      case "hex":
        return fromBytes(decodeHex(text));
      case "base64":
      case "base64url":
        return fromBytes(decodeBase64(text));
      case "latin1":
      case "ascii": {
        const out = rawAlloc(text.length);
        for (let i = 0; i < text.length; i++) out[i] = text.charCodeAt(i) & 0xff;
        return out;
      }
      case "utf16le": {
        const out = rawAlloc(text.length * 2);
        for (let i = 0; i < text.length; i++) {
          const unit = text.charCodeAt(i);
          out[i * 2] = unit & 0xff;
          out[i * 2 + 1] = unit >> 8;
        }
        return out;
      }
      default:
        return rawFrom(text);
    }
  }

  function byteLengthOf(text: string, encoding: string): number {
    switch (encoding) {
      case "utf8":
        return rawByteLength(text);
      case "hex":
        return text.length >>> 1;
      case "base64":
      case "base64url": {
        let length = text.length;
        while (length > 0 && text.charCodeAt(length - 1) === 61) length--;
        return (length * 3) >>> 2;
      }
      case "utf16le":
        return text.length * 2;
      default:
        return text.length;
    }
  }

  function toInteger(value: any, fallback: number): number {
    if (value === undefined) return fallback;
    const n = Number(value);
    if (Number.isNaN(n)) return 0;
    return Math.trunc(n);
  }

  function rangeError(name: string, message: string): Error {
    const error: any = new RangeError(message);
    error.code = name;
    return error;
  }

  function checkOffset(offset: any, width: number, length: number): number {
    if (typeof offset !== "number") {
      const error: any = new TypeError(
        `The "offset" argument must be of type number. Received ${typeof offset}`,
      );
      error.code = "ERR_INVALID_ARG_TYPE";
      throw error;
    }
    if (!Number.isInteger(offset)) {
      throw rangeError("ERR_OUT_OF_RANGE", `The value of "offset" is out of range. It must be an integer. Received ${offset}`);
    }
    if (offset < 0 || offset + width > length) {
      if (length - width < 0) throw rangeError("ERR_BUFFER_OUT_OF_BOUNDS", "Attempt to access memory outside buffer bounds");
      throw rangeError(
        "ERR_OUT_OF_RANGE",
        `The value of "offset" is out of range. It must be >= 0 and <= ${length - width}. Received ${offset}`,
      );
    }
    return offset;
  }

  function checkValue(value: any, min: number, max: number, name: string): void {
    if (typeof value !== "number") {
      const error: any = new TypeError(`The "value" argument must be of type number. Received ${typeof value}`);
      error.code = "ERR_INVALID_ARG_TYPE";
      throw error;
    }
    if (value < min || value > max) {
      throw rangeError(
        "ERR_OUT_OF_RANGE",
        `The value of "value" is out of range. It must be >= ${min} and <= ${max}. Received ${name === "int" ? value : value}`,
      );
    }
  }

  function readUInt(buf: Uint8Array, offset: number, width: number, littleEndian: boolean): number {
    let value = 0;
    if (littleEndian) {
      for (let i = width - 1; i >= 0; i--) value = value * 256 + buf[offset + i];
    } else {
      for (let i = 0; i < width; i++) value = value * 256 + buf[offset + i];
    }
    return value;
  }

  function writeUInt(buf: Uint8Array, value: number, offset: number, width: number, littleEndian: boolean): number {
    let remaining = value;
    if (littleEndian) {
      for (let i = 0; i < width; i++) {
        buf[offset + i] = remaining & 0xff;
        remaining = Math.floor(remaining / 256);
      }
    } else {
      for (let i = width - 1; i >= 0; i--) {
        buf[offset + i] = remaining & 0xff;
        remaining = Math.floor(remaining / 256);
      }
    }
    return offset + width;
  }

  const tagOf = Object.prototype.toString;
  function isArrayBufferLike(value: any): boolean {
    if (value === null || typeof value !== "object") return false;
    const tag = tagOf.call(value);
    return tag === "[object ArrayBuffer]" || tag === "[object SharedArrayBuffer]";
  }
  function isUint8(value: any): boolean {
    return rawIsBuffer(value) || tagOf.call(value) === "[object Uint8Array]";
  }

  function toBuffer(value: any, name: string): Uint8Array {
    if (isUint8(value)) return value;
    const error: any = new TypeError(
      `The "${name}" argument must be an instance of Buffer or Uint8Array. Received ${value === null ? "null" : typeof value}`,
    );
    error.code = "ERR_INVALID_ARG_TYPE";
    throw error;
  }

  function compareBytes(a: Uint8Array, aStart: number, aEnd: number, b: Uint8Array, bStart: number, bEnd: number): number {
    const aLength = aEnd - aStart;
    const bLength = bEnd - bStart;
    const length = aLength < bLength ? aLength : bLength;
    for (let i = 0; i < length; i++) {
      const x = a[aStart + i];
      const y = b[bStart + i];
      if (x !== y) return x < y ? -1 : 1;
    }
    return aLength < bLength ? -1 : aLength > bLength ? 1 : 0;
  }

  function needleBytes(value: any, encoding: string): Uint8Array {
    if (typeof value === "string") return encodeString(value, encoding);
    if (typeof value === "number") return fromBytes([value & 0xff]);
    return toBuffer(value, "value");
  }

  function indexOfBytes(buf: Uint8Array, needle: Uint8Array, from: number, last: boolean): number {
    const length = buf.length;
    const n = needle.length;
    if (n === 0) return from > length ? length : from;
    if (!last) {
      for (let i = from < 0 ? 0 : from; i + n <= length; i++) {
        let j = 0;
        while (j < n && buf[i + j] === needle[j]) j++;
        if (j === n) return i;
      }
    } else {
      for (let i = from > length - n ? length - n : from; i >= 0; i--) {
        let j = 0;
        while (j < n && buf[i + j] === needle[j]) j++;
        if (j === n) return i;
      }
    }
    return -1;
  }

  function searchFrom(buf: Uint8Array, byteOffset: any, last: boolean): { from: number; encoding: string } {
    let encoding = "utf8";
    let from: number;
    if (typeof byteOffset === "string") {
      encoding = checkEncoding(byteOffset);
      from = last ? buf.length : 0;
    } else {
      from = toInteger(byteOffset, last ? buf.length : 0);
      if (byteOffset !== undefined && Number.isNaN(Number(byteOffset))) from = last ? buf.length : 0;
      if (from < 0) from += buf.length;
    }
    return { from, encoding };
  }

  const proto = Object.create(U8.prototype);

  // One descriptor for every member, not a literal per call: `defineProperty` reads its fields and
  // keeps nothing, and a literal per member was garbage in every realm's bootstrap (realms issue 08).
  const memberDescriptor: PropertyDescriptor = { value: undefined, writable: true, configurable: true, enumerable: false };
  function define(name: string, value: any): void {
    memberDescriptor.value = value;
    Object.defineProperty(proto, name, memberDescriptor);
    memberDescriptor.value = undefined;
  }

  define("constructor", B);

  define("toString", function toString(this: Uint8Array, encoding?: any, start?: any, end?: any): string {
    const length = this.length;
    if (length === 0) return "";
    let from = toInteger(start, 0);
    let to = toInteger(end, length);
    if (from < 0) from = 0;
    if (to > length) to = length;
    if (to <= from) return "";
    const normalized = checkEncoding(encoding);
    const view = from === 0 && to === length ? this : this.subarray(from, to);
    switch (normalized) {
      case "hex":
      case "base64":
      case "utf8":
        return rawToString.call(view, normalized);
      case "base64url": {
        const text = rawToString.call(view, "base64");
        let out = "";
        for (let i = 0; i < text.length; i++) {
          const ch = text[i];
          out += ch === "+" ? "-" : ch === "/" ? "_" : ch === "=" ? "" : ch;
        }
        return out;
      }
      case "utf16le": {
        let text = "";
        for (let i = 0; i + 1 < view.length; i += 2) text += String.fromCharCode(view[i] | (view[i + 1] << 8));
        return text;
      }
      case "ascii": {
        let text = "";
        for (let i = 0; i < view.length; i++) text += String.fromCharCode(view[i] & 0x7f);
        return text;
      }
      case "latin1": {
        let text = "";
        for (let i = 0; i < view.length; i++) text += String.fromCharCode(view[i]);
        return text;
      }
      default:
        return rawToString.call(view, normalized);
    }
  });

  define("toLocaleString", proto.toString);

  function toArray(view: ArrayLike<number>): number[] {
    const data: number[] = [];
    for (let i = 0; i < view.length; i++) data.push(view[i]);
    return data;
  }

  define("toJSON", function toJSON(this: Uint8Array) {
    return { type: "Buffer", data: toArray(this) };
  });

  define("equals", function equals(this: Uint8Array, other: any): boolean {
    const b = toBuffer(other, "otherBuffer");
    if (this === b) return true;
    if (this.length !== b.length) return false;
    return compareBytes(this, 0, this.length, b, 0, b.length) === 0;
  });

  define("compare", function compare(this: Uint8Array, target: any, targetStart?: any, targetEnd?: any, sourceStart?: any, sourceEnd?: any): number {
    const b = toBuffer(target, "target");
    const ts = toInteger(targetStart, 0);
    const te = toInteger(targetEnd, b.length);
    const ss = toInteger(sourceStart, 0);
    const se = toInteger(sourceEnd, this.length);
    if (ts < 0 || ss < 0 || te > b.length || se > this.length) {
      throw rangeError("ERR_OUT_OF_RANGE", "The value of \"targetStart\" is out of range.");
    }
    return compareBytes(this, ss, se < ss ? ss : se, b, ts, te < ts ? ts : te);
  });

  define("copy", function copy(this: Uint8Array, target: any, targetStart?: any, sourceStart?: any, sourceEnd?: any): number {
    const t = toBuffer(target, "target");
    let ts = toInteger(targetStart, 0);
    let ss = toInteger(sourceStart, 0);
    let se = toInteger(sourceEnd, this.length);
    if (ts < 0) throw rangeError("ERR_OUT_OF_RANGE", `The value of "targetStart" is out of range. It must be >= 0. Received ${ts}`);
    if (ss < 0) throw rangeError("ERR_OUT_OF_RANGE", `The value of "sourceStart" is out of range. It must be >= 0. Received ${ss}`);
    if (se > this.length) se = this.length;
    if (ts >= t.length || ss >= se) return 0;
    let count = se - ss;
    if (count > t.length - ts) count = t.length - ts;
    if (t.buffer === this.buffer) {
      // Overlapping views over one allocation: copy through a temporary rather than in place.
      const temp = this.slice(ss, ss + count);
      for (let i = 0; i < count; i++) t[ts + i] = temp[i];
    } else {
      for (let i = 0; i < count; i++) t[ts + i] = this[ss + i];
    }
    return count;
  });

  define("fill", function fill(this: Uint8Array, value: any, offset?: any, end?: any, encoding?: any): Uint8Array {
    if (typeof offset === "string") {
      encoding = offset;
      offset = 0;
      end = this.length;
    } else if (typeof end === "string") {
      encoding = end;
      end = this.length;
    }
    let from = toInteger(offset, 0);
    let to = toInteger(end, this.length);
    if (from < 0 || to > this.length) throw rangeError("ERR_OUT_OF_RANGE", "The value of \"offset\" is out of range.");
    if (to <= from) return this;
    let pattern: Uint8Array;
    if (typeof value === "string") {
      pattern = encodeString(value, checkEncoding(encoding));
      if (pattern.length === 0) {
        if (value.length === 0) pattern = fromBytes([0]);
        else throw rangeError("ERR_INVALID_ARG_VALUE", `The argument 'value' is invalid. Received '${value}'`);
      }
    } else if (typeof value === "number") {
      pattern = fromBytes([value & 0xff]);
    } else if (typeof value === "boolean") {
      pattern = fromBytes([value ? 1 : 0]);
    } else {
      pattern = toBuffer(value, "value");
      if (pattern.length === 0) throw rangeError("ERR_INVALID_ARG_VALUE", "The argument 'value' is invalid.");
    }
    for (let i = from, j = 0; i < to; i++, j++) {
      if (j === pattern.length) j = 0;
      this[i] = pattern[j];
    }
    return this;
  });

  define("indexOf", function indexOf(this: Uint8Array, value: any, byteOffset?: any, encoding?: any): number {
    const search = searchFrom(this, byteOffset, false);
    const needle = needleBytes(value, encoding !== undefined ? checkEncoding(encoding) : search.encoding);
    return indexOfBytes(this, needle, search.from, false);
  });

  define("lastIndexOf", function lastIndexOf(this: Uint8Array, value: any, byteOffset?: any, encoding?: any): number {
    const search = searchFrom(this, byteOffset, true);
    const needle = needleBytes(value, encoding !== undefined ? checkEncoding(encoding) : search.encoding);
    return indexOfBytes(this, needle, search.from, true);
  });

  define("includes", function includes(this: Uint8Array, value: any, byteOffset?: any, encoding?: any): boolean {
    return proto.indexOf.call(this, value, byteOffset, encoding) !== -1;
  });

  function sliceOf(buf: Uint8Array, start: any, end: any): Uint8Array {
    const length = buf.length;
    let from = toInteger(start, 0);
    let to = toInteger(end, length);
    if (from < 0) from = from + length < 0 ? 0 : from + length;
    else if (from > length) from = length;
    if (to < 0) to = to + length < 0 ? 0 : to + length;
    else if (to > length) to = length;
    if (to < from) to = from;
    return wrap(buf.buffer, buf.byteOffset + from, to - from);
  }

  // Both share memory: node's `slice` is `subarray` with a deprecation note, not a copy.
  define("subarray", function subarray(this: Uint8Array, start?: any, end?: any): Uint8Array {
    return sliceOf(this, start, end);
  });
  define("slice", function slice(this: Uint8Array, start?: any, end?: any): Uint8Array {
    return sliceOf(this, start, end);
  });

  define("write", function write(this: Uint8Array, text: any, offset?: any, length?: any, encoding?: any): number {
    if (typeof text !== "string") {
      const error: any = new TypeError(`The "string" argument must be of type string. Received ${typeof text}`);
      error.code = "ERR_INVALID_ARG_TYPE";
      throw error;
    }
    if (typeof offset === "string") {
      encoding = offset;
      offset = 0;
      length = this.length;
    } else if (typeof length === "string") {
      encoding = length;
      length = undefined;
    }
    const at = toInteger(offset, 0);
    if (at < 0 || at > this.length) throw rangeError("ERR_OUT_OF_RANGE", `The value of "offset" is out of range. It must be >= 0 && <= ${this.length}. Received ${at}`);
    let room = this.length - at;
    const wanted = toInteger(length, room);
    if (wanted < room) room = wanted < 0 ? 0 : wanted;
    const normalized = checkEncoding(encoding);
    let bytes = encodeString(text, normalized);
    if (bytes.length > room) {
      // A UTF-8 write never splits a character: back up to the last boundary that fits.
      let cut = room;
      if (normalized === "utf8") {
        while (cut > 0 && (bytes[cut] & 0xc0) === 0x80) cut--;
      } else if (normalized === "utf16le") {
        cut &= ~1;
      }
      bytes = bytes.subarray(0, cut);
    }
    for (let i = 0; i < bytes.length; i++) this[at + i] = bytes[i];
    return bytes.length;
  });

  define("swap16", function swap16(this: Uint8Array): Uint8Array {
    if (this.length % 2 !== 0) throw rangeError("ERR_INVALID_BUFFER_SIZE", "Buffer size must be a multiple of 16-bits");
    for (let i = 0; i < this.length; i += 2) {
      const t = this[i];
      this[i] = this[i + 1];
      this[i + 1] = t;
    }
    return this;
  });
  define("swap32", function swap32(this: Uint8Array): Uint8Array {
    if (this.length % 4 !== 0) throw rangeError("ERR_INVALID_BUFFER_SIZE", "Buffer size must be a multiple of 32-bits");
    for (let i = 0; i < this.length; i += 4) {
      let t = this[i]; this[i] = this[i + 3]; this[i + 3] = t;
      t = this[i + 1]; this[i + 1] = this[i + 2]; this[i + 2] = t;
    }
    return this;
  });
  define("swap64", function swap64(this: Uint8Array): Uint8Array {
    if (this.length % 8 !== 0) throw rangeError("ERR_INVALID_BUFFER_SIZE", "Buffer size must be a multiple of 64-bits");
    for (let i = 0; i < this.length; i += 8) {
      for (let j = 0; j < 4; j++) {
        const t = this[i + j];
        this[i + j] = this[i + 7 - j];
        this[i + 7 - j] = t;
      }
    }
    return this;
  });

  // The integer readers and writers, each a width and a byte order.
  const widths: [string, number][] = [["8", 1], ["16", 2], ["32", 4]];
  for (const [bits, width] of widths) {
    const limit = 2 ** (width * 8);
    const orders: [string, boolean][] = width === 1 ? [["", false]] : [["LE", true], ["BE", false]];
    for (const [order, littleEndian] of orders) {
      define(`readUInt${bits}${order}`, function (this: Uint8Array, offset: any = 0): number {
        checkOffset(offset, width, this.length);
        return readUInt(this, offset, width, littleEndian);
      });
      define(`readInt${bits}${order}`, function (this: Uint8Array, offset: any = 0): number {
        checkOffset(offset, width, this.length);
        const unsigned = readUInt(this, offset, width, littleEndian);
        return unsigned >= limit / 2 ? unsigned - limit : unsigned;
      });
      define(`writeUInt${bits}${order}`, function (this: Uint8Array, value: any, offset: any = 0): number {
        checkOffset(offset, width, this.length);
        checkValue(value, 0, limit - 1, "uint");
        return writeUInt(this, value, offset, width, littleEndian);
      });
      define(`writeInt${bits}${order}`, function (this: Uint8Array, value: any, offset: any = 0): number {
        checkOffset(offset, width, this.length);
        checkValue(value, -limit / 2, limit / 2 - 1, "int");
        return writeUInt(this, value < 0 ? value + limit : value, offset, width, littleEndian);
      });
    }
  }
  // The lowercase `Uint` spellings node keeps as aliases.
  // An index loop, not `for...of`, which makes an iterator result per name in every realm's
  // bootstrap (realms issue 08).
  const upperNames = Object.getOwnPropertyNames(proto);
  for (let i = 0; i < upperNames.length; i++) {
    const name = upperNames[i];
    if (name.indexOf("UInt") !== -1) define(name.replace("UInt", "Uint"), proto[name]);
  }

  // Variable-width: `readUIntBE(offset, byteLength)` up to six bytes, exact in a double.
  function checkByteLength(byteLength: any): number {
    if (typeof byteLength !== "number" || !Number.isInteger(byteLength) || byteLength < 1 || byteLength > 6) {
      throw rangeError("ERR_OUT_OF_RANGE", `The value of "byteLength" is out of range. It must be >= 1 and <= 6. Received ${byteLength}`);
    }
    return byteLength;
  }
  for (const [order, littleEndian] of [["LE", true], ["BE", false]] as [string, boolean][]) {
    define(`readUInt${order}`, function (this: Uint8Array, offset: any, byteLength: any): number {
      const width = checkByteLength(byteLength);
      checkOffset(offset, width, this.length);
      return readUInt(this, offset, width, littleEndian);
    });
    define(`readInt${order}`, function (this: Uint8Array, offset: any, byteLength: any): number {
      const width = checkByteLength(byteLength);
      checkOffset(offset, width, this.length);
      const limit = 2 ** (width * 8);
      const unsigned = readUInt(this, offset, width, littleEndian);
      return unsigned >= limit / 2 ? unsigned - limit : unsigned;
    });
    define(`writeUInt${order}`, function (this: Uint8Array, value: any, offset: any, byteLength: any): number {
      const width = checkByteLength(byteLength);
      checkOffset(offset, width, this.length);
      checkValue(value, 0, 2 ** (width * 8) - 1, "uint");
      return writeUInt(this, value, offset, width, littleEndian);
    });
    define(`writeInt${order}`, function (this: Uint8Array, value: any, offset: any, byteLength: any): number {
      const width = checkByteLength(byteLength);
      checkOffset(offset, width, this.length);
      const limit = 2 ** (width * 8);
      checkValue(value, -limit / 2, limit / 2 - 1, "int");
      return writeUInt(this, value < 0 ? value + limit : value, offset, width, littleEndian);
    });
    define(`readUint${order}`, proto[`readUInt${order}`]);
    define(`writeUint${order}`, proto[`writeUInt${order}`]);

    // Floats through a shared view, so the bit pattern is the platform's and the byte order
    // is chosen here rather than assumed.
    define(`readFloat${order}`, function (this: Uint8Array, offset: any = 0): number {
      checkOffset(offset, 4, this.length);
      for (let i = 0; i < 4; i++) F32_BYTES[littleEndian === LITTLE_ENDIAN ? i : 3 - i] = this[offset + i];
      return F32[0];
    });
    define(`readDouble${order}`, function (this: Uint8Array, offset: any = 0): number {
      checkOffset(offset, 8, this.length);
      for (let i = 0; i < 8; i++) F64_BYTES[littleEndian === LITTLE_ENDIAN ? i : 7 - i] = this[offset + i];
      return F64[0];
    });
    define(`writeFloat${order}`, function (this: Uint8Array, value: any, offset: any = 0): number {
      checkOffset(offset, 4, this.length);
      F32[0] = Number(value);
      for (let i = 0; i < 4; i++) this[offset + i] = F32_BYTES[littleEndian === LITTLE_ENDIAN ? i : 3 - i];
      return offset + 4;
    });
    define(`writeDouble${order}`, function (this: Uint8Array, value: any, offset: any = 0): number {
      checkOffset(offset, 8, this.length);
      F64[0] = Number(value);
      for (let i = 0; i < 8; i++) this[offset + i] = F64_BYTES[littleEndian === LITTLE_ENDIAN ? i : 7 - i];
      return offset + 8;
    });

    // 64-bit integers are BigInts; two 32-bit halves keep every bit.
    define(`readBigUInt64${order}`, function (this: Uint8Array, offset: any = 0): bigint {
      checkOffset(offset, 8, this.length);
      const lo = readUInt(this, littleEndian ? offset : offset + 4, 4, littleEndian);
      const hi = readUInt(this, littleEndian ? offset + 4 : offset, 4, littleEndian);
      return (BigInt(hi) << 32n) | BigInt(lo);
    });
    define(`readBigInt64${order}`, function (this: Uint8Array, offset: any = 0): bigint {
      const unsigned: bigint = proto[`readBigUInt64${order}`].call(this, offset);
      return unsigned >= 1n << 63n ? unsigned - (1n << 64n) : unsigned;
    });
    // No type check on `value`: node has none either, and a Number reaches the `&` below,
    // which is the "Cannot mix BigInt and other types" TypeError a program sees there.
    define(`writeBigUInt64${order}`, function (this: Uint8Array, value: any, offset: any = 0): number {
      checkOffset(offset, 8, this.length);
      if (value < 0n || value > (1n << 64n) - 1n) {
        throw rangeError("ERR_OUT_OF_RANGE", `The value of "value" is out of range. It must be >= 0n and < 2n ** 64n. Received ${value}`);
      }
      const lo = Number(value & 0xffffffffn);
      const hi = Number(value >> 32n);
      writeUInt(this, lo, littleEndian ? offset : offset + 4, 4, littleEndian);
      writeUInt(this, hi, littleEndian ? offset + 4 : offset, 4, littleEndian);
      return offset + 8;
    });
    define(`writeBigInt64${order}`, function (this: Uint8Array, value: any, offset: any = 0): number {
      checkOffset(offset, 8, this.length);
      if (value < -(1n << 63n) || value > (1n << 63n) - 1n) {
        throw rangeError("ERR_OUT_OF_RANGE", `The value of "value" is out of range. It must be >= -(2n ** 63n) and < 2n ** 63n. Received ${value}`);
      }
      const unsigned = value & ((1n << 64n) - 1n);
      return proto[`writeBigUInt64${order}`].call(this, unsigned, offset);
    });
    define(`readBigUint64${order}`, proto[`readBigUInt64${order}`]);
    define(`writeBigUint64${order}`, proto[`writeBigUInt64${order}`]);
  }

  // `%TypedArray%.prototype.map` and `filter` answer through the species constructor, which
  // for a Buffer is `Buffer`: the result is a Buffer too.
  define("map", function map(this: Uint8Array, callback: any, thisArg?: any): Uint8Array {
    const out: Uint8Array = U8.prototype.map.call(this, callback, thisArg);
    return wrap(out.buffer, out.byteOffset, out.length);
  });
  define("filter", function filter(this: Uint8Array, callback: any, thisArg?: any): Uint8Array {
    const out: Uint8Array = U8.prototype.filter.call(this, callback, thisArg);
    return wrap(out.buffer, out.byteOffset, out.length);
  });

  define("inspect", function inspect(this: Uint8Array): string {
    let hex = "";
    const shown = this.length < 50 ? this.length : 50;
    for (let i = 0; i < shown; i++) hex += (i ? " " : "") + (this[i] < 16 ? "0" : "") + this[i].toString(16);
    return `<Buffer ${hex}${this.length > 50 ? ` ... ${this.length - 50} more bytes` : ""}>`;
  });

  // The statics the engines have, made whole: `from` honours an encoding and takes an
  // ArrayBuffer window, `alloc` fills with a pattern, `concat` and `compare` read the prototype.
  B.from = function from(value: any, encodingOrOffset?: any, length?: any): Uint8Array {
    if (typeof value === "string") return encodeString(value, checkEncoding(encodingOrOffset));
    if (isArrayBufferLike(value)) {
      const offset = toInteger(encodingOrOffset, 0);
      const bytes = value.byteLength;
      if (offset < 0 || offset > bytes) throw rangeError("ERR_BUFFER_OUT_OF_BOUNDS", '"offset" is outside of buffer bounds');
      const count = length === undefined ? bytes - offset : toInteger(length, 0);
      if (count < 0 || offset + count > bytes) throw rangeError("ERR_BUFFER_OUT_OF_BOUNDS", '"length" is outside of buffer bounds');
      return wrap(value, offset, count);
    }
    if (isUint8(value) || ArrayBuffer.isView(value)) {
      const bytes = isUint8(value) ? value : new U8(value.buffer, value.byteOffset, value.byteLength);
      const out = rawAlloc(bytes.length);
      for (let i = 0; i < bytes.length; i++) out[i] = bytes[i];
      return out;
    }
    if (Array.isArray(value)) return rawFrom(value);
    if (value !== null && typeof value === "object") {
      if (value.type === "Buffer" && Array.isArray(value.data)) return rawFrom(value.data);
      if (typeof value.valueOf === "function") {
        const primitive = value.valueOf();
        if (primitive !== value && (typeof primitive === "string" || typeof primitive === "object")) {
          return B.from(primitive, encodingOrOffset, length);
        }
      }
      if (typeof value[Symbol.toPrimitive] === "function") {
        return B.from(value[Symbol.toPrimitive]("string"), encodingOrOffset, length);
      }
      if (typeof value.length === "number") return rawFrom(toArray(value));
    }
    const error: any = new TypeError(
      `The first argument must be of type string or an instance of Buffer, ArrayBuffer, or Array or an Array-like Object. Received ${value === null ? "null" : typeof value}`,
    );
    error.code = "ERR_INVALID_ARG_TYPE";
    throw error;
  };

  B.alloc = function alloc(size: any, fill?: any, encoding?: any): Uint8Array {
    if (typeof size !== "number") {
      const error: any = new TypeError(`The "size" argument must be of type number. Received ${typeof size}`);
      error.code = "ERR_INVALID_ARG_TYPE";
      throw error;
    }
    const out = rawAlloc(size);
    if (fill !== undefined && fill !== 0 && size > 0) proto.fill.call(out, fill, 0, size, encoding);
    return out;
  };
  B.allocUnsafe = B.alloc;
  B.allocUnsafeSlow = B.alloc;

  B.byteLength = function byteLength(value: any, encoding?: any): number {
    if (typeof value === "string") return byteLengthOf(value, checkEncoding(encoding));
    if (ArrayBuffer.isView(value) || isArrayBufferLike(value)) return value.byteLength;
    const error: any = new TypeError(
      `The "string" argument must be of type string or an instance of Buffer or ArrayBuffer. Received ${typeof value}`,
    );
    error.code = "ERR_INVALID_ARG_TYPE";
    throw error;
  };

  B.concat = function concat(list: any, totalLength?: any): Uint8Array {
    if (!Array.isArray(list)) {
      const error: any = new TypeError('The "list" argument must be an instance of Array.');
      error.code = "ERR_INVALID_ARG_TYPE";
      throw error;
    }
    let total = 0;
    for (let i = 0; i < list.length; i++) total += toBuffer(list[i], `list[${i}]`).length;
    if (totalLength !== undefined) total = toInteger(totalLength, total);
    const out = rawAlloc(total);
    let at = 0;
    for (let i = 0; i < list.length && at < total; i++) {
      const item = list[i];
      const count = item.length < total - at ? item.length : total - at;
      for (let j = 0; j < count; j++) out[at + j] = item[j];
      at += count;
    }
    return out;
  };

  B.compare = function compare(a: any, b: any): number {
    return proto.compare.call(toBuffer(a, "buf1"), toBuffer(b, "buf2"));
  };

  B.isEncoding = function isEncoding(encoding: any): boolean {
    return typeof encoding === "string" && normalizeEncoding(encoding) !== "";
  };

  B.copyBytesFrom = function copyBytesFrom(view: any, offset?: any, length?: any): Uint8Array {
    const bytes = new U8(view.buffer, view.byteOffset, view.byteLength);
    const from = toInteger(offset, 0) * view.BYTES_PER_ELEMENT;
    const to = length === undefined ? bytes.length : from + toInteger(length, 0) * view.BYTES_PER_ELEMENT;
    return B.from(bytes.subarray(from, to));
  };

  B.poolSize = 8192;

  B.prototype = proto;
})();
