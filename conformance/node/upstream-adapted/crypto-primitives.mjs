import crypto, {
  randomBytes,
  randomFillSync,
  getHashes,
  createHash,
  createHmac,
  getRandomValues,
  timingSafeEqual,
} from 'node:crypto';

// 1. getHashes
const hashes = getHashes();
if (!hashes.includes('sha256') || !hashes.includes('md5')) {
  throw new Error('getHashes missing standard digests');
}

// 2. createHash
const hash = createHash('sha256').update('hello world').digest('hex');
if (hash !== 'b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9') {
  throw new Error('createHash sha256 mismatch');
}

// 3. createHmac (RFC 4231)
const hmac = createHmac('sha256', 'key').update('The quick brown fox jumps over the lazy dog').digest('hex');
if (hmac !== 'f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8') {
  throw new Error('createHmac sha256 mismatch');
}

// 4. randomBytes
const bytes = randomBytes(16);
if (bytes.length !== 16) {
  throw new Error('randomBytes length mismatch');
}

// 5. randomFillSync
const buf = new Uint8Array(8);
randomFillSync(buf, 2, 4);
if (buf[0] !== 0 || buf[1] !== 0 || buf[6] !== 0 || buf[7] !== 0) {
  throw new Error('randomFillSync offset/size respected');
}

// 6. default export object equality / presence
if (typeof crypto.createHmac !== 'function' || typeof crypto.randomBytes !== 'function') {
  throw new Error('crypto default export missing functions');
}

// 7. timingSafeEqual
const b1 = Buffer.from([1, 2, 3, 4]);
const b2 = Buffer.from([1, 2, 3, 4]);
const b3 = Buffer.from([1, 2, 3, 5]);
const b4 = Buffer.from([1, 2, 3]);

if (!timingSafeEqual(b1, b2)) throw new Error('timingSafeEqual eq');
if (timingSafeEqual(b1, b3)) throw new Error('timingSafeEqual neq');

let threwDiffLen = false;
try {
  timingSafeEqual(b1, b4);
} catch {
  threwDiffLen = true;
}
if (!threwDiffLen) throw new Error('timingSafeEqual diff len must throw');

let threwType = false;
try {
  timingSafeEqual(b1, 'not a buffer');
} catch {
  threwType = true;
}
if (!threwType) throw new Error('timingSafeEqual non-buffer must throw');

// 8. getRandomValues
const arr = new Uint8Array(16);
const res = getRandomValues(arr);
if (res !== arr || res.length !== 16) throw new Error('getRandomValues Uint8Array');

let threwFloat = false;
try {
  getRandomValues(new Float32Array(4));
} catch {
  threwFloat = true;
}
if (!threwFloat) throw new Error('getRandomValues Float32Array must throw');

let threwBig = false;
try {
  getRandomValues(new Uint8Array(65537));
} catch {
  threwBig = true;
}
if (!threwBig) throw new Error('getRandomValues > 65536 must throw');

// 9. global crypto object
if (typeof globalThis.crypto !== 'object') throw new Error('globalThis.crypto object');
if (typeof globalThis.crypto.getRandomValues !== 'function') throw new Error('globalThis.crypto.getRandomValues');
if (typeof globalThis.crypto.randomUUID !== 'function') throw new Error('globalThis.crypto.randomUUID');
const uuid = globalThis.crypto.randomUUID();
if (typeof uuid !== 'string' || uuid.length !== 36) throw new Error('randomUUID format');

console.log('pass');
