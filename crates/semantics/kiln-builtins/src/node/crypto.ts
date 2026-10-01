// Deterministic node:crypto implementation (Phase 5.2, ADR 0005)

function rotr32(n: number, b: number): number {
  return (n >>> b) | (n << (32 - b));
}

function rotl32(n: number, b: number): number {
  return (n << b) | (n >>> (32 - b));
}

const SHA256_K: number[] = [
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2
];

function sha256Bytes(bytes: Uint8Array): Uint8Array {
  const l = bytes.length;
  const bitLen = l * 8;
  const padLen = (l % 64 < 56) ? (56 - (l % 64)) : (120 - (l % 64));
  const total = l + padLen + 8;
  const padded = new Uint8Array(total);
  padded.set(bytes, 0);
  padded[l] = 0x80;
  const view = new DataView(padded.buffer);
  view.setUint32(total - 4, bitLen >>> 0);
  view.setUint32(total - 8, Math.floor(bitLen / 0x100000000));

  let h0 = 0x6a09e667, h1 = 0xbb67ae85, h2 = 0x3c6ef372, h3 = 0xa54ff53a;
  let h4 = 0x510e527f, h5 = 0x9b05688c, h6 = 0x1f83d9ab, h7 = 0x5be0cd19;
  const W = new Int32Array(64);

  for (let i = 0; i < total; i += 64) {
    for (let j = 0; j < 16; j++) {
      W[j] = view.getInt32(i + j * 4);
    }
    for (let j = 16; j < 64; j++) {
      const s0 = rotr32(W[j - 15], 7) ^ rotr32(W[j - 15], 18) ^ (W[j - 15] >>> 3);
      const s1 = rotr32(W[j - 2], 17) ^ rotr32(W[j - 2], 19) ^ (W[j - 2] >>> 10);
      W[j] = (W[j - 16] + s0 + W[j - 7] + s1) | 0;
    }
    let a = h0, b = h1, c = h2, d = h3, e = h4, f = h5, g = h6, h = h7;
    for (let j = 0; j < 64; j++) {
      const S1 = rotr32(e, 6) ^ rotr32(e, 11) ^ rotr32(e, 25);
      const ch = (e & f) ^ ((~e) & g);
      const temp1 = (h + S1 + ch + SHA256_K[j] + W[j]) | 0;
      const S0 = rotr32(a, 2) ^ rotr32(a, 13) ^ rotr32(a, 22);
      const maj = (a & b) ^ (a & c) ^ (b & c);
      const temp2 = (S0 + maj) | 0;
      h = g; g = f; f = e; e = (d + temp1) | 0;
      d = c; c = b; b = a; a = (temp1 + temp2) | 0;
    }
    h0 = (h0 + a) | 0; h1 = (h1 + b) | 0; h2 = (h2 + c) | 0; h3 = (h3 + d) | 0;
    h4 = (h4 + e) | 0; h5 = (h5 + f) | 0; h6 = (h6 + g) | 0; h7 = (h7 + h) | 0;
  }

  const out = new Uint8Array(32);
  const outView = new DataView(out.buffer);
  outView.setInt32(0, h0); outView.setInt32(4, h1); outView.setInt32(8, h2); outView.setInt32(12, h3);
  outView.setInt32(16, h4); outView.setInt32(20, h5); outView.setInt32(24, h6); outView.setInt32(28, h7);
  return out;
}

function md5Bytes(bytes: Uint8Array): Uint8Array {
  const l = bytes.length;
  const bitLen = l * 8;
  const padLen = (l % 64 < 56) ? (56 - (l % 64)) : (120 - (l % 64));
  const total = l + padLen + 8;
  const padded = new Uint8Array(total);
  padded.set(bytes, 0);
  padded[l] = 0x80;
  const view = new DataView(padded.buffer);
  view.setUint32(total - 8, bitLen >>> 0, true);
  view.setUint32(total - 4, Math.floor(bitLen / 0x100000000), true);

  let a = 0x67452301, b = 0xefcdab89, c = 0x98badcfe, d = 0x10325476;
  const S = [
    7, 12, 17, 22,  7, 12, 17, 22,  7, 12, 17, 22,  7, 12, 17, 22,
    5,  9, 14, 20,  5,  9, 14, 20,  5,  9, 14, 20,  5,  9, 14, 20,
    4, 11, 16, 23,  4, 11, 16, 23,  4, 11, 16, 23,  4, 11, 16, 23,
    6, 10, 15, 21,  6, 10, 15, 21,  6, 10, 15, 21,  6, 10, 15, 21
  ];
  const K = new Int32Array(64);
  for (let i = 0; i < 64; i++) {
    K[i] = Math.floor(Math.abs(Math.sin(i + 1)) * 0x100000000) | 0;
  }
  const M = new Int32Array(16);
  for (let i = 0; i < total; i += 64) {
    for (let j = 0; j < 16; j++) {
      M[j] = view.getInt32(i + j * 4, true);
    }
    let aa = a, bb = b, cc = c, dd = d;
    for (let j = 0; j < 64; j++) {
      let f: number;
      let g: number;
      if (j < 16) {
        f = (b & c) | ((~b) & d);
        g = j;
      } else if (j < 32) {
        f = (d & b) | ((~d) & c);
        g = (5 * j + 1) % 16;
      } else if (j < 48) {
        f = b ^ c ^ d;
        g = (3 * j + 5) % 16;
      } else {
        f = c ^ (b | (~d));
        g = (7 * j) % 16;
      }
      const temp = d;
      d = c;
      c = b;
      const sum = (a + f + K[j] + M[g]) | 0;
      const rot = (sum << S[j]) | (sum >>> (32 - S[j]));
      b = (b + rot) | 0;
      a = temp;
    }
    a = (a + aa) | 0;
    b = (b + bb) | 0;
    c = (c + cc) | 0;
    d = (d + dd) | 0;
  }
  const out = new Uint8Array(16);
  const outView = new DataView(out.buffer);
  outView.setInt32(0, a, true);
  outView.setInt32(4, b, true);
  outView.setInt32(8, c, true);
  outView.setInt32(12, d, true);
  return out;
}

function sha1Bytes(bytes: Uint8Array): Uint8Array {
  const l = bytes.length;
  const bitLen = l * 8;
  const padLen = (l % 64 < 56) ? (56 - (l % 64)) : (120 - (l % 64));
  const total = l + padLen + 8;
  const padded = new Uint8Array(total);
  padded.set(bytes, 0);
  padded[l] = 0x80;
  const view = new DataView(padded.buffer);
  view.setUint32(total - 4, bitLen >>> 0);
  view.setUint32(total - 8, Math.floor(bitLen / 0x100000000));

  let h0 = 0x67452301, h1 = 0xefcdab89, h2 = 0x98badcfe, h3 = 0x10325476, h4 = 0xc3d2e1f0;
  const W = new Int32Array(80);
  for (let i = 0; i < total; i += 64) {
    for (let t = 0; t < 16; t++) {
      W[t] = view.getInt32(i + t * 4);
    }
    for (let t = 16; t < 80; t++) {
      W[t] = rotl32(W[t - 3] ^ W[t - 8] ^ W[t - 14] ^ W[t - 16], 1);
    }
    let a = h0, b = h1, c = h2, d = h3, e = h4;
    for (let t = 0; t < 80; t++) {
      let f: number;
      let k: number;
      if (t < 20) {
        f = (b & c) | ((~b) & d);
        k = 0x5a827999;
      } else if (t < 40) {
        f = b ^ c ^ d;
        k = 0x6ed9eba1;
      } else if (t < 60) {
        f = (b & c) | (b & d) | (c & d);
        k = 0x8f1bbcdc;
      } else {
        f = b ^ c ^ d;
        k = 0xca62c1d6;
      }
      const temp = (rotl32(a, 5) + f + e + k + W[t]) | 0;
      e = d;
      d = c;
      c = rotl32(b, 30);
      b = a;
      a = temp;
    }
    h0 = (h0 + a) | 0;
    h1 = (h1 + b) | 0;
    h2 = (h2 + c) | 0;
    h3 = (h3 + d) | 0;
    h4 = (h4 + e) | 0;
  }
  const out = new Uint8Array(20);
  const outView = new DataView(out.buffer);
  outView.setInt32(0, h0); outView.setInt32(4, h1); outView.setInt32(8, h2); outView.setInt32(12, h3); outView.setInt32(16, h4);
  return out;
}

function computeHmac(algo: string, keyBytes: Uint8Array, dataBytes: Uint8Array): Uint8Array {
  const blockSize = 64;
  let k = keyBytes;
  if (k.length > blockSize) {
    if (algo === 'md5') k = md5Bytes(k);
    else if (algo === 'sha1') k = sha1Bytes(k);
    else k = sha256Bytes(k);
  }
  const kPadded = new Uint8Array(blockSize);
  kPadded.set(k, 0);
  const ipad = new Uint8Array(blockSize + dataBytes.length);
  for (let i = 0; i < blockSize; i++) {
    ipad[i] = kPadded[i] ^ 0x36;
  }
  ipad.set(dataBytes, blockSize);

  let inner: Uint8Array;
  if (algo === 'md5') inner = md5Bytes(ipad);
  else if (algo === 'sha1') inner = sha1Bytes(ipad);
  else inner = sha256Bytes(ipad);

  const opad = new Uint8Array(blockSize + inner.length);
  for (let i = 0; i < blockSize; i++) {
    opad[i] = kPadded[i] ^ 0x5c;
  }
  opad.set(inner, blockSize);

  if (algo === 'md5') return md5Bytes(opad);
  if (algo === 'sha1') return sha1Bytes(opad);
  return sha256Bytes(opad);
}

function toUint8Array(data: any, encoding?: string): Uint8Array {
  if (typeof data === 'string') {
    if (typeof Buffer !== 'undefined') {
      return Buffer.from(data, (encoding as any) || 'utf8');
    }
    return new TextEncoder().encode(data);
  }
  if (data instanceof Uint8Array) {
    return data;
  }
  if (data && data.buffer instanceof ArrayBuffer) {
    return new Uint8Array(data.buffer, data.byteOffset || 0, data.byteLength);
  }
  return new Uint8Array(data);
}

function toHex(bytes: Uint8Array): string {
  let hex = '';
  const hexChars = '0123456789abcdef';
  for (let i = 0; i < bytes.length; i++) {
    hex += hexChars[bytes[i] >> 4] + hexChars[bytes[i] & 0x0f];
  }
  return hex;
}

function toBase64(bytes: Uint8Array, urlSafe: boolean = false): string {
  const chars = urlSafe
    ? 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_'
    : 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  let result = '';
  const len = bytes.length;
  let i = 0;
  for (; i + 2 < len; i += 3) {
    const b0 = bytes[i];
    const b1 = bytes[i + 1];
    const b2 = bytes[i + 2];
    result += chars[b0 >> 2];
    result += chars[((b0 & 3) << 4) | (b1 >> 4)];
    result += chars[((b1 & 15) << 2) | (b2 >> 6)];
    result += chars[b2 & 63];
  }
  if (i < len) {
    const b0 = bytes[i];
    result += chars[b0 >> 2];
    if (i + 1 < len) {
      const b1 = bytes[i + 1];
      result += chars[((b0 & 3) << 4) | (b1 >> 4)];
      result += chars[(b1 & 15) << 2];
      if (!urlSafe) result += '=';
    } else {
      result += chars[(b0 & 3) << 4];
      if (!urlSafe) result += '==';
    }
  }
  return result;
}

function encodeDigest(raw: Uint8Array, encoding?: string): any {
  if (!encoding) {
    if (typeof Buffer !== 'undefined') {
      return Buffer.from(raw);
    }
    return raw;
  }
  const norm = String(encoding).toLowerCase();
  if (norm === 'hex') {
    return toHex(raw);
  }
  if (norm === 'base64') {
    return toBase64(raw, false);
  }
  if (norm === 'base64url') {
    return toBase64(raw, true);
  }
  if (norm === 'utf8' || norm === 'utf-8') {
    return new TextDecoder('utf-8').decode(raw);
  }
  if (norm === 'latin1' || norm === 'binary') {
    let s = '';
    for (let i = 0; i < raw.length; i++) {
      s += String.fromCharCode(raw[i]);
    }
    return s;
  }
  if (typeof Buffer !== 'undefined') {
    return Buffer.from(raw).toString(encoding as any);
  }
  throw new TypeError(`Unknown encoding: ${encoding}`);
}

export class Hash {
  private _algorithm: string;
  private _buffers: Uint8Array[] = [];

  constructor(algorithm: string) {
    const norm = String(algorithm || '').toLowerCase().replace(/-/g, '');
    if (norm !== 'sha256' && norm !== 'md5' && norm !== 'sha1' && norm !== 'sha512') {
      throw new TypeError(`Digest method not supported: ${algorithm}`);
    }
    this._algorithm = norm;
  }

  update(data: any, encoding?: string): this {
    this._buffers.push(toUint8Array(data, encoding));
    return this;
  }

  digest(encoding?: string): any {
    let totalLen = 0;
    for (const b of this._buffers) totalLen += b.length;
    const combined = new Uint8Array(totalLen);
    let offset = 0;
    for (const b of this._buffers) {
      combined.set(b, offset);
      offset += b.length;
    }
    let raw: Uint8Array;
    if (this._algorithm === 'md5') raw = md5Bytes(combined);
    else if (this._algorithm === 'sha1') raw = sha1Bytes(combined);
    else raw = sha256Bytes(combined);

    return encodeDigest(raw, encoding);
  }
}

const cryptoKeyImportBridge = (globalThis as any).__kilnCryptoKeyImport;
const cryptoKeyPublicBridge = (globalThis as any).__kilnCryptoKeyPublic;
const cryptoKeyInfoBridge = (globalThis as any).__kilnCryptoKeyInfo;
const cryptoKeyExportBridge = (globalThis as any).__kilnCryptoKeyExport;
const cryptoKeyFreeBridge = (globalThis as any).__kilnCryptoKeyFree;
delete (globalThis as any).__kilnCryptoKeyImport;
delete (globalThis as any).__kilnCryptoKeyPublic;
delete (globalThis as any).__kilnCryptoKeyInfo;
delete (globalThis as any).__kilnCryptoKeyExport;
delete (globalThis as any).__kilnCryptoKeyFree;

const asymmetricKeyHandles = new WeakMap<KeyObject, any>();
const asymmetricKeyFinalizer: FinalizationRegistry<any> | undefined =
  typeof FinalizationRegistry === 'function' && typeof cryptoKeyFreeBridge === 'function'
    ? new FinalizationRegistry((handle: any) => { cryptoKeyFreeBridge(handle); })
    : undefined;

/** Node's opaque key handle. Asymmetric values carry a parsed key-core handle in a WeakMap;
 * private material is never stored as an ordinary JavaScript byte property. */
export class KeyObject {
  readonly type: string;
  readonly symmetricKeySize: number | undefined;
  readonly asymmetricKeyType: string | undefined;
  readonly asymmetricKeyDetails: any;
  private _key: Uint8Array;

  constructor(key: any, type: string = 'secret', asymmetricKeyType?: string, asymmetricKeyDetails?: any) {
    this.type = type;
    this._key = type === 'secret' ? toUint8Array(key).slice() : new Uint8Array(0);
    this.symmetricKeySize = type === 'secret' ? this._key.length : undefined;
    this.asymmetricKeyType = asymmetricKeyType;
    this.asymmetricKeyDetails = asymmetricKeyDetails;
  }

  get [Symbol.toStringTag](): string { return 'KeyObject'; }

  export(options?: any): any {
    const handle = asymmetricKeyHandles.get(this);
    if (handle !== undefined) {
      if (options === undefined || options === null || typeof options !== 'object') {
        throw new TypeError('The "options.format" property is required');
      }
      const requestedFormat = options.format;
      if (requestedFormat === undefined) throw new TypeError('The "options.format" property is required');
      const format = String(requestedFormat);
      if (format === 'jwk') {
        const bytes = cryptoKeyExportBridge(handle, 'jwk', 'json');
        return JSON.parse(Buffer.from(bytes).toString('utf8'));
      }
      if (format !== 'der' && format !== 'pem') {
        throw new TypeError(`The key export format "${format}" is not supported`);
      }
      if (options.type === undefined) {
        throw new TypeError('The "options.type" property is required for PEM and DER export');
      }
      const type = String(options.type);
      const validTypes = this.type === 'private' ? ['pkcs1', 'pkcs8'] : ['pkcs1', 'spki'];
      if (!validTypes.includes(type)) {
        throw new TypeError(`The key export type "${type}" is invalid for a ${this.type} key`);
      }
      const bytes = cryptoKeyExportBridge(handle, type, format);
      return format === 'pem' ? Buffer.from(bytes).toString('utf8') : Buffer.from(bytes);
    }
    if (options !== undefined) {
      throw new TypeError('The options argument is not supported for secret keys');
    }
    return Buffer.from(this._key);
  }
}

function makeAsymmetricKey(handle: any): KeyObject {
  if (asymmetricKeyFinalizer === undefined) {
    if (typeof cryptoKeyFreeBridge === 'function') cryptoKeyFreeBridge(handle);
    throw new TypeError('The runtime cannot release asymmetric key handles after collection');
  }
  const info = cryptoKeyInfoBridge(handle);
  const key = new KeyObject(undefined, info.type, info.asymmetricKeyType, info.asymmetricKeyDetails);
  try {
    asymmetricKeyHandles.set(key, handle);
    asymmetricKeyFinalizer.register(key, handle);
  } catch (error) {
    asymmetricKeyHandles.delete(key);
    cryptoKeyFreeBridge(handle);
    throw error;
  }
  return key;
}

export function createSecretKey(key: any, encoding?: string): KeyObject {
  if (typeof key === 'string') {
    return new KeyObject(toUint8Array(key, encoding), 'secret');
  }
  if (key === undefined || key === null || typeof key !== 'object' || encoding !== undefined) {
    throw new TypeError('The "key" argument must be of type string or an instance of Buffer or TypedArray');
  }
  return new KeyObject(key, 'secret');
}

function importKeyInput(input: any): { bytes: Uint8Array; format: string; type?: string } {
  let key = input;
  let format = 'pem';
  let encoding: string | undefined;
  let type: string | undefined;
  if (key && typeof key === 'object' && !(key instanceof Uint8Array) && !(key instanceof KeyObject) && 'key' in key) {
    format = key.format === undefined ? 'pem' : String(key.format);
    encoding = key.encoding === undefined ? undefined : String(key.encoding);
    type = key.type === undefined ? undefined : String(key.type);
    key = key.key;
  }
  if (key instanceof KeyObject) {
    throw new TypeError(`The key argument must be key material, not a ${key.type} KeyObject`);
  }
  if (format === 'jwk') {
    if (!key || typeof key !== 'object') throw new TypeError('The key argument must be an RSA JWK object');
    return { bytes: new TextEncoder().encode(JSON.stringify(key)), format, type };
  }
  if (format !== 'pem' && format !== 'der') {
    throw new TypeError(`The key format "${format}" is not supported`);
  }
  const bytes = typeof key === 'string'
    ? toUint8Array(key, encoding)
    : key instanceof Uint8Array
      ? key
      : key && key.buffer instanceof ArrayBuffer
        ? new Uint8Array(key.buffer, key.byteOffset || 0, key.byteLength)
        : undefined;
  if (!bytes) throw new TypeError('The key argument must be a string, Buffer, TypedArray, or key options object');
  return { bytes, format, type };
}

export function createPrivateKey(input: any): KeyObject {
  const { bytes, format, type } = importKeyInput(input);
  if (format === 'der' && type !== 'pkcs1' && type !== 'pkcs8') {
    throw new TypeError('The "key.type" property must be "pkcs1" or "pkcs8" for a private DER key');
  }
  return makeAsymmetricKey(cryptoKeyImportBridge(bytes, format, 'private', type || ''));
}

export function createPublicKey(input: any): KeyObject {
  if (input instanceof KeyObject) {
    const handle = asymmetricKeyHandles.get(input);
    if (handle === undefined) throw new TypeError('The key argument must be an asymmetric KeyObject');
    if (input.type !== 'private') {
      throw new TypeError('The "key" argument must be a private KeyObject');
    }
    return makeAsymmetricKey(cryptoKeyPublicBridge(handle));
  }
  const { bytes, format, type } = importKeyInput(input);
  if (format === 'der' && type !== 'pkcs1' && type !== 'spki') {
    throw new TypeError('The "key.type" property must be "pkcs1" or "spki" for a public DER key');
  }
  const imported = cryptoKeyImportBridge(bytes, format, format === 'pem' ? 'auto' : 'public', type || '');
  try {
    return makeAsymmetricKey(cryptoKeyPublicBridge(imported));
  } finally {
    cryptoKeyFreeBridge(imported);
  }
}

export class Hmac {
  private _algorithm: string;
  private _key: Uint8Array;
  private _buffers: Uint8Array[] = [];

  constructor(algorithm: string, key: any) {
    if (key === undefined || key === null) {
      throw new TypeError('The "key" argument must be of type string or an instance of Buffer or TypedArray');
    }
    if (key instanceof KeyObject && key.type !== 'secret') {
      throw new TypeError(`Invalid key object type ${key.type}, expected secret`);
    }
    const norm = String(algorithm || '').toLowerCase().replace(/-/g, '');
    if (norm !== 'sha256' && norm !== 'md5' && norm !== 'sha1' && norm !== 'sha512') {
      throw new TypeError(`Digest method not supported: ${algorithm}`);
    }
    this._algorithm = norm;
    this._key = toUint8Array(key instanceof KeyObject ? key.export() : key);
  }

  update(data: any, encoding?: string): this {
    this._buffers.push(toUint8Array(data, encoding));
    return this;
  }

  digest(encoding?: string): any {
    let totalLen = 0;
    for (const b of this._buffers) totalLen += b.length;
    const combined = new Uint8Array(totalLen);
    let offset = 0;
    for (const b of this._buffers) {
      combined.set(b, offset);
      offset += b.length;
    }
    const raw = computeHmac(this._algorithm, this._key, combined);
    return encodeDigest(raw, encoding);
  }
}

export function createHash(algorithm: string): Hash {
  return new Hash(algorithm);
}

export function createHmac(algorithm: string, key: any): Hmac {
  return new Hmac(algorithm, key);
}

export function getHashes(): string[] {
  return ['md5', 'sha1', 'sha256', 'sha512'];
}

export function randomUUID(): string {
  const s = '0123456789abcdef';
  let uuid = '';
  for (let i = 0; i < 36; i++) {
    if (i === 8 || i === 13 || i === 18 || i === 23) {
      uuid += '-';
    } else if (i === 14) {
      uuid += '4';
    } else if (i === 19) {
      uuid += s[(Math.floor(Math.random() * 16) & 0x3) | 0x8];
    } else {
      uuid += s[Math.floor(Math.random() * 16)];
    }
  }
  return uuid;
}

export function getRandomValues(typedArray: any): any {
  if (!typedArray || typeof typedArray !== 'object') {
    throw new TypeError('The "typedArray" argument must be an instance of TypedArray');
  }
  const name = typedArray.constructor ? typedArray.constructor.name : '';
  if (name === 'Float32Array' || name === 'Float64Array') {
    throw new TypeError('The "typedArray" argument must be an integer TypedArray');
  }
  if (typedArray.byteLength > 65536) {
    throw new RangeError('The requested length exceeds 65,536 bytes');
  }
  const view = new Uint8Array(typedArray.buffer, typedArray.byteOffset, typedArray.byteLength);
  for (let i = 0; i < view.length; i++) {
    view[i] = Math.floor(Math.random() * 256);
  }
  return typedArray;
}

export function randomBytes(size: number, callback?: (err: Error | null, buf?: any) => void): any {
  const s = Number(size);
  if (isNaN(s) || s < 0 || s > 2147483647) {
    throw new RangeError('The value of "size" is out of range');
  }
  const bytes = new Uint8Array(s);
  for (let i = 0; i < s; i++) {
    bytes[i] = Math.floor(Math.random() * 256);
  }
  const result = typeof Buffer !== 'undefined' ? Buffer.from(bytes) : bytes;
  if (typeof callback === 'function') {
    callback(null, result);
    return;
  }
  return result;
}

export function randomFillSync(buffer: any, offset: number = 0, size?: number): any {
  if (!buffer || typeof buffer !== 'object' || (!ArrayBuffer.isView(buffer) && !(buffer instanceof ArrayBuffer))) {
    throw new TypeError('The "buffer" argument must be an ArrayBufferView or ArrayBuffer');
  }
  const totalLen = buffer.byteLength !== undefined ? buffer.byteLength : buffer.length;
  const off = Number(offset || 0);
  if (isNaN(off) || off < 0 || off > totalLen) {
    throw new RangeError('The value of "offset" is out of range');
  }
  const sz = size !== undefined ? Number(size) : totalLen - off;
  if (isNaN(sz) || sz < 0 || off + sz > totalLen) {
    throw new RangeError('The value of "size" is out of range');
  }
  const view = buffer instanceof Uint8Array ? buffer : new Uint8Array(buffer.buffer || buffer, buffer.byteOffset || 0, totalLen);
  for (let i = 0; i < sz; i++) {
    view[off + i] = Math.floor(Math.random() * 256);
  }
  return buffer;
}

export function timingSafeEqual(a: any, b: any): boolean {
  if (!a || !b || typeof a !== 'object' || typeof b !== 'object') {
    throw new TypeError('The "a" and "b" arguments must be of type ArrayBufferView or ArrayBuffer');
  }
  const byteLenA = a.byteLength !== undefined ? a.byteLength : (a.length !== undefined ? a.length : -1);
  const byteLenB = b.byteLength !== undefined ? b.byteLength : (b.length !== undefined ? b.length : -1);
  if (byteLenA === -1 || byteLenB === -1) {
    throw new TypeError('The "a" and "b" arguments must be of type ArrayBufferView or ArrayBuffer');
  }
  if (byteLenA !== byteLenB) {
    throw new RangeError('Input buffers must have the same length');
  }
  const viewA = new Uint8Array(a.buffer || a, a.byteOffset || 0, byteLenA);
  const viewB = new Uint8Array(b.buffer || b, b.byteOffset || 0, byteLenB);
  let out = 0;
  for (let i = 0; i < byteLenA; i++) {
    out |= viewA[i] ^ viewB[i];
  }
  return out === 0;
}

const webcryptoValue = typeof globalThis !== 'undefined'
  ? (globalThis as any).crypto
  : undefined;
export const webcrypto = webcryptoValue;
export const subtle = webcryptoValue ? webcryptoValue.subtle : undefined;

const cryptoDefault = {
  Hash,
  Hmac,
  KeyObject,
  createPublicKey,
  createSecretKey,
  createPrivateKey,
  createHash,
  createHmac,
  getHashes,
  getRandomValues,
  randomBytes,
  randomFillSync,
  randomUUID,
  timingSafeEqual,
  subtle,
  webcrypto,
};

// The global `crypto`. The interpreter installs its own, which already carries every one of
// these; the native backend installs a namespace that refuses every member by name until this
// module is linked, and this is where it gets linked. The binding is *replaced* rather than
// patched: a write into a native built-in's store bumps `kiln_c99_builtin_store_epoch` and
// switches built-in speculation off for the whole program, and a plain object costs nothing.
// `globalThis.crypto === require("node:crypto")` is false in node too. The test is `in` rather
// than a read, because a read of an unlinked member on the native namespace refuses -- that is
// the point of it -- while `in` answers false.
if (typeof globalThis !== 'undefined' && globalThis.crypto &&
    !('createHash' in (globalThis as any).crypto)) {
  (globalThis as any).crypto = { ...cryptoDefault, default: cryptoDefault };
}

export default cryptoDefault;
