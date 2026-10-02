// kiln-builtins: node:zlib implementation (Phase 5.2, ADR 0005)

export const constants: Record<string, number> = {
  Z_NO_FLUSH: 0,
  Z_PARTIAL_FLUSH: 1,
  Z_SYNC_FLUSH: 2,
  Z_FULL_FLUSH: 3,
  Z_FINISH: 4,
  Z_BLOCK: 5,
  Z_TREES: 6,
  Z_OK: 0,
  Z_STREAM_END: 1,
  Z_NEED_DICT: 2,
  Z_ERRNO: -1,
  Z_STREAM_ERROR: -2,
  Z_DATA_ERROR: -3,
  Z_MEM_ERROR: -4,
  Z_BUF_ERROR: -5,
  Z_VERSION_ERROR: -6,
  Z_NO_COMPRESSION: 0,
  Z_BEST_SPEED: 1,
  Z_BEST_COMPRESSION: 9,
  Z_DEFAULT_COMPRESSION: -1,
  Z_FILTERED: 1,
  Z_HUFFMAN_ONLY: 2,
  Z_RLE: 3,
  Z_FIXED: 4,
  Z_DEFAULT_STRATEGY: 0,
  DEFLATE: 1,
  INFLATE: 2,
  GZIP: 3,
  GUNZIP: 4,
  DEFLATERAW: 5,
  INFLATERAW: 6,
  UNZIP: 7,
  BROTLI_DECODE: 8,
  BROTLI_ENCODE: 9,
};

const CRC_TABLE = new Uint32Array(256);
for (let i = 0; i < 256; i++) {
  let c = i;
  for (let j = 0; j < 8; j++) {
    c = (c & 1) ? (0xedb88320 ^ (c >>> 1)) : (c >>> 1);
  }
  CRC_TABLE[i] = c >>> 0;
}

function crc32(buf: Uint8Array): number {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) {
    c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}

function adler32(buf: Uint8Array): number {
  let s1 = 1;
  let s2 = 0;
  for (let i = 0; i < buf.length; i++) {
    s1 = (s1 + buf[i]) % 65521;
    s2 = (s2 + s1) % 65521;
  }
  return ((s2 << 16) | s1) >>> 0;
}

function toUint8Array(input: any): Uint8Array {
  if (input instanceof Uint8Array) {
    return input;
  }
  if (typeof input === "string") {
    const encoder = new TextEncoder();
    return encoder.encode(input);
  }
  if (input && input.buffer instanceof ArrayBuffer) {
    return new Uint8Array(input.buffer, input.byteOffset || 0, input.byteLength || 0);
  }
  if (ArrayBuffer.isView(input)) {
    return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
  }
  if (input instanceof ArrayBuffer) {
    return new Uint8Array(input);
  }
  return new Uint8Array(0);
}

function toBuffer(arr: Uint8Array): any {
  if (typeof (globalThis as any).Buffer !== "undefined" && typeof (globalThis as any).Buffer.from === "function") {
    return (globalThis as any).Buffer.from(arr);
  }
  return arr;
}

// RFC 1951 Deflate compression using uncompressed (stored) blocks
function rawDeflate(bytes: Uint8Array): Uint8Array {
  const len = bytes.length;
  if (len === 0) {
    return new Uint8Array([0x01, 0x00, 0x00, 0xff, 0xff]);
  }
  const chunks: Uint8Array[] = [];
  let offset = 0;
  while (offset < len) {
    const chunkLen = Math.min(65535, len - offset);
    const isFinal = (offset + chunkLen === len) ? 1 : 0;
    const header = new Uint8Array(5);
    header[0] = isFinal;
    header[1] = chunkLen & 0xff;
    header[2] = (chunkLen >> 8) & 0xff;
    const nlen = (~chunkLen) & 0xffff;
    header[3] = nlen & 0xff;
    header[4] = (nlen >> 8) & 0xff;
    chunks.push(header);
    chunks.push(bytes.subarray(offset, offset + chunkLen));
    offset += chunkLen;
  }
  let totalLen = 0;
  for (let i = 0; i < chunks.length; i++) totalLen += chunks[i].length;
  const out = new Uint8Array(totalLen);
  let pos = 0;
  for (let i = 0; i < chunks.length; i++) {
    out.set(chunks[i], pos);
    pos += chunks[i].length;
  }
  return out;
}

export function deflateSync(input: any, _options?: any): any {
  const bytes = toUint8Array(input);
  const def = rawDeflate(bytes);
  const out = new Uint8Array(2 + def.length + 4);
  out[0] = 0x78;
  out[1] = 0x01;
  out.set(def, 2);
  const adler = adler32(bytes);
  const off = 2 + def.length;
  out[off] = (adler >>> 24) & 0xff;
  out[off + 1] = (adler >>> 16) & 0xff;
  out[off + 2] = (adler >>> 8) & 0xff;
  out[off + 3] = adler & 0xff;
  return toBuffer(out);
}

export function gzipSync(input: any, _options?: any): any {
  const bytes = toUint8Array(input);
  const def = rawDeflate(bytes);
  const out = new Uint8Array(10 + def.length + 8);
  out[0] = 0x1f;
  out[1] = 0x8b;
  out[2] = 0x08;
  out[3] = 0x00;
  out[8] = 0x00;
  out[9] = 0xff;
  out.set(def, 10);
  const crc = crc32(bytes);
  const off = 10 + def.length;
  out[off] = crc & 0xff;
  out[off + 1] = (crc >>> 8) & 0xff;
  out[off + 2] = (crc >>> 16) & 0xff;
  out[off + 3] = (crc >>> 24) & 0xff;
  const isize = bytes.length >>> 0;
  out[off + 4] = isize & 0xff;
  out[off + 5] = (isize >>> 8) & 0xff;
  out[off + 6] = (isize >>> 16) & 0xff;
  out[off + 7] = (isize >>> 24) & 0xff;
  return toBuffer(out);
}

class BitReader {
  buf: Uint8Array;
  pos: number;
  bitBuf: number;
  bitsInBuf: number;

  constructor(buf: Uint8Array, pos: number = 0) {
    this.buf = buf;
    this.pos = pos;
    this.bitBuf = 0;
    this.bitsInBuf = 0;
  }

  readBits(n: number): number {
    while (this.bitsInBuf < n) {
      if (this.pos >= this.buf.length) {
        throw new Error("Unexpected end of compressed stream");
      }
      this.bitBuf |= this.buf[this.pos++] << this.bitsInBuf;
      this.bitsInBuf += 8;
    }
    const val = this.bitBuf & ((1 << n) - 1);
    this.bitBuf >>>= n;
    this.bitsInBuf -= n;
    return val;
  }

  align(): void {
    this.bitBuf = 0;
    this.bitsInBuf = 0;
  }

  readBytes(n: number): Uint8Array {
    this.align();
    if (this.pos + n > this.buf.length) {
      throw new Error("Unexpected end of compressed stream");
    }
    const sub = this.buf.subarray(this.pos, this.pos + n);
    this.pos += n;
    return sub;
  }
}

class HuffmanTree {
  maxBits: number;
  table: Int32Array;
  lengths: Uint8Array;

  constructor(lengths: Uint8Array) {
    let max = 0;
    for (let i = 0; i < lengths.length; i++) {
      if (lengths[i] > max) max = lengths[i];
    }
    this.maxBits = max;
    if (max === 0) {
      this.table = new Int32Array(0);
      this.lengths = new Uint8Array(0);
      return;
    }
    const count = new Uint16Array(max + 1);
    for (let i = 0; i < lengths.length; i++) {
      count[lengths[i]]++;
    }
    const nextCode = new Uint16Array(max + 1);
    let code = 0;
    for (let bits = 1; bits <= max; bits++) {
      code = (code + count[bits - 1]) << 1;
      nextCode[bits] = code;
    }
    this.table = new Int32Array(1 << max);
    this.table.fill(-1);
    this.lengths = new Uint8Array(1 << max);
    for (let i = 0; i < lengths.length; i++) {
      const len = lengths[i];
      if (len !== 0) {
        const c = nextCode[len]++;
        let rev = 0;
        for (let b = 0; b < len; b++) {
          rev = (rev << 1) | ((c >> b) & 1);
        }
        const fillStep = 1 << len;
        for (let j = rev; j < (1 << max); j += fillStep) {
          this.table[j] = i;
          this.lengths[j] = len;
        }
      }
    }
  }

  decode(reader: BitReader): number {
    while (reader.bitsInBuf < this.maxBits && reader.pos < reader.buf.length) {
      reader.bitBuf |= reader.buf[reader.pos++] << reader.bitsInBuf;
      reader.bitsInBuf += 8;
    }
    const mask = (1 << this.maxBits) - 1;
    const lookup = reader.bitBuf & mask;
    const sym = this.table[lookup];
    if (sym === -1) {
      const availableBits = reader.bitsInBuf;
      for (let bits = 1; bits <= availableBits; bits++) {
        const subLookup = reader.bitBuf & ((1 << bits) - 1);
        for (let j = 0; j < this.table.length; j++) {
          if (this.lengths[j] === bits && (j & ((1 << bits) - 1)) === subLookup) {
            reader.bitBuf >>>= bits;
            reader.bitsInBuf -= bits;
            return this.table[j];
          }
        }
      }
      throw new Error("Invalid Huffman code");
    }
    const len = this.lengths[lookup];
    reader.bitBuf >>>= len;
    reader.bitsInBuf -= len;
    return sym;
  }
}

const FIXED_LIT_LENGTHS = new Uint8Array(288);
for (let i = 0; i <= 143; i++) FIXED_LIT_LENGTHS[i] = 8;
for (let i = 144; i <= 255; i++) FIXED_LIT_LENGTHS[i] = 9;
for (let i = 256; i <= 279; i++) FIXED_LIT_LENGTHS[i] = 7;
for (let i = 280; i <= 287; i++) FIXED_LIT_LENGTHS[i] = 8;
const FIXED_LIT_TREE = new HuffmanTree(FIXED_LIT_LENGTHS);

const FIXED_DIST_LENGTHS = new Uint8Array(32);
FIXED_DIST_LENGTHS.fill(5);
const FIXED_DIST_TREE = new HuffmanTree(FIXED_DIST_LENGTHS);

const LENGTH_BASE = [
  3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31,
  35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258
];
const LENGTH_EXTRA_BITS = [
  0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2,
  3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0
];

const DIST_BASE = [
  1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193,
  257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577
];
const DIST_EXTRA_BITS = [
  0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6,
  7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13
];

const CODE_LENGTH_ORDER = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

function rawInflate(reader: BitReader): Uint8Array {
  const output: number[] = [];
  let isFinal = 0;
  while (!isFinal) {
    isFinal = reader.readBits(1);
    const btype = reader.readBits(2);
    if (btype === 0) {
      reader.align();
      const len = reader.readBits(16);
      const nlen = reader.readBits(16);
      if ((len ^ 0xffff) !== nlen) {
        throw new Error("Invalid stored block length");
      }
      const data = reader.readBytes(len);
      for (let i = 0; i < data.length; i++) output.push(data[i]);
    } else if (btype === 1 || btype === 2) {
      let litTree: HuffmanTree;
      let distTree: HuffmanTree;
      if (btype === 1) {
        litTree = FIXED_LIT_TREE;
        distTree = FIXED_DIST_TREE;
      } else {
        const hlit = reader.readBits(5) + 257;
        const hdist = reader.readBits(5) + 1;
        const hclen = reader.readBits(4) + 4;
        const codeLengths = new Uint8Array(19);
        for (let i = 0; i < hclen; i++) {
          codeLengths[CODE_LENGTH_ORDER[i]] = reader.readBits(3);
        }
        const clTree = new HuffmanTree(codeLengths);
        const totalCodes = hlit + hdist;
        const allLengths = new Uint8Array(totalCodes);
        let idx = 0;
        while (idx < totalCodes) {
          const sym = clTree.decode(reader);
          if (sym < 16) {
            allLengths[idx++] = sym;
          } else if (sym === 16) {
            const copy = allLengths[idx - 1];
            const repeat = reader.readBits(2) + 3;
            for (let r = 0; r < repeat; r++) allLengths[idx++] = copy;
          } else if (sym === 17) {
            const repeat = reader.readBits(3) + 3;
            for (let r = 0; r < repeat; r++) allLengths[idx++] = 0;
          } else if (sym === 18) {
            const repeat = reader.readBits(7) + 11;
            for (let r = 0; r < repeat; r++) allLengths[idx++] = 0;
          }
        }
        litTree = new HuffmanTree(allLengths.subarray(0, hlit));
        distTree = new HuffmanTree(allLengths.subarray(hlit));
      }

      while (true) {
        const sym = litTree.decode(reader);
        if (sym < 256) {
          output.push(sym);
        } else if (sym === 256) {
          break;
        } else {
          const lenIdx = sym - 257;
          const length = LENGTH_BASE[lenIdx] + reader.readBits(LENGTH_EXTRA_BITS[lenIdx]);
          const distSym = distTree.decode(reader);
          const distance = DIST_BASE[distSym] + reader.readBits(DIST_EXTRA_BITS[distSym]);
          const start = output.length - distance;
          for (let i = 0; i < length; i++) {
            output.push(output[start + i]);
          }
        }
      }
    } else {
      throw new Error("Reserved block type: " + btype);
    }
  }
  return new Uint8Array(output);
}

export function inflateSync(input: any, _options?: any): any {
  const bytes = toUint8Array(input);
  let start = 0;
  if (bytes.length >= 2 && (bytes[0] & 0x0f) === 8 && ((bytes[0] * 256 + bytes[1]) % 31 === 0)) {
    start = 2;
    if (bytes[1] & 0x20) {
      start += 4;
    }
  }
  const reader = new BitReader(bytes, start);
  const out = rawInflate(reader);
  return toBuffer(out);
}

export function gunzipSync(input: any, _options?: any): any {
  const bytes = toUint8Array(input);
  if (bytes.length < 10 || bytes[0] !== 0x1f || bytes[1] !== 0x8b) {
    throw new Error("Invalid gzip header");
  }
  const flg = bytes[3];
  let pos = 10;
  if (flg & 0x04) {
    const xlen = bytes[pos] | (bytes[pos + 1] << 8);
    pos += 2 + xlen;
  }
  if (flg & 0x08) {
    while (bytes[pos++] !== 0);
  }
  if (flg & 0x10) {
    while (bytes[pos++] !== 0);
  }
  if (flg & 0x02) {
    pos += 2;
  }
  const reader = new BitReader(bytes, pos);
  const out = rawInflate(reader);
  return toBuffer(out);
}

export function unzipSync(input: any, _options?: any): any {
  const bytes = toUint8Array(input);
  if (bytes.length >= 2 && bytes[0] === 0x1f && bytes[1] === 0x8b) {
    return gunzipSync(bytes);
  }
  return inflateSync(bytes);
}

class ZlibTransform {
  _transformFn: (data: Uint8Array) => any;
  _chunks: Uint8Array[];
  _output: any;
  _events: Record<string, Function[]>;
  writableEnded: boolean;
  writableFinished: boolean;
  readableEnded: boolean;
  destroyed: boolean;

  constructor(transformFn: (data: Uint8Array) => any) {
    this._transformFn = transformFn;
    this._chunks = [];
    this._output = null;
    this._events = Object.create(null);
    this.writableEnded = false;
    this.writableFinished = false;
    this.readableEnded = false;
    this.destroyed = false;
  }

  on(event: string, listener: Function): this {
    (this._events[event] = this._events[event] || []).push(listener);
    return this;
  }

  once(event: string, listener: Function): this {
    const g = (...args: any[]) => {
      this.removeListener(event, g);
      listener.apply(this, args);
    };
    return this.on(event, g);
  }

  removeListener(event: string, listener: Function): this {
    const list = this._events[event];
    if (list) {
      const idx = list.indexOf(listener);
      if (idx !== -1) list.splice(idx, 1);
    }
    return this;
  }

  emit(event: string, ...args: any[]): boolean {
    const list = this._events[event];
    if (!list || list.length === 0) return false;
    const copy = list.slice();
    for (let i = 0; i < copy.length; i++) {
      copy[i].apply(this, args);
    }
    return true;
  }

  write(chunk: any, encoding?: any, cb?: any): boolean {
    if (typeof encoding === "function") {
      cb = encoding;
      encoding = undefined;
    }
    if (chunk) {
      this._chunks.push(toUint8Array(chunk));
    }
    if (cb) cb();
    return true;
  }

  end(chunk?: any, encoding?: any, cb?: any): this {
    if (typeof chunk === "function") {
      cb = chunk;
      chunk = undefined;
    } else if (typeof encoding === "function") {
      cb = encoding;
      encoding = undefined;
    }
    if (chunk) this.write(chunk, encoding);
    this.writableEnded = true;
    this.writableFinished = true;
    let totalLen = 0;
    for (let i = 0; i < this._chunks.length; i++) totalLen += this._chunks[i].length;
    const combined = new Uint8Array(totalLen);
    let pos = 0;
    for (let i = 0; i < this._chunks.length; i++) {
      combined.set(this._chunks[i], pos);
      pos += this._chunks[i].length;
    }
    const res = this._transformFn(combined);
    this._output = res;
    this.readableEnded = true;
    this.emit("data", res);
    this.emit("end");
    this.emit("finish");
    if (cb) cb();
    return this;
  }

  read(): any {
    const out = this._output;
    this._output = null;
    return out;
  }

  pipe(destination: any, _options?: any): any {
    this.on("data", (chunk: any) => {
      if (destination && typeof destination.write === "function") {
        destination.write(chunk);
      }
    });
    this.on("end", () => {
      if (destination && typeof destination.end === "function") {
        destination.end();
      }
    });
    return destination;
  }

  destroy(error?: any): this {
    this.destroyed = true;
    if (error) this.emit("error", error);
    this.emit("close");
    return this;
  }
}

export function createDeflate(options?: any): ZlibTransform {
  return new ZlibTransform((data) => deflateSync(data, options));
}

export function createInflate(options?: any): ZlibTransform {
  return new ZlibTransform((data) => inflateSync(data, options));
}

export function createGzip(options?: any): ZlibTransform {
  return new ZlibTransform((data) => gzipSync(data, options));
}

export function createGunzip(options?: any): ZlibTransform {
  return new ZlibTransform((data) => gunzipSync(data, options));
}

export function createUnzip(options?: any): ZlibTransform {
  return new ZlibTransform((data) => unzipSync(data, options));
}

export function createBrotliDecompress(_options?: any): ZlibTransform {
  return new ZlibTransform(() => {
    throw new Error("brotli decompression not yet supported");
  });
}

const zlibModule = {
  constants,
  createBrotliDecompress,
  createDeflate,
  createGunzip,
  createGzip,
  createInflate,
  createUnzip,
  deflateSync,
  gunzipSync,
  gzipSync,
  inflateSync,
  unzipSync,
  ...constants,
};

export default zlibModule;
