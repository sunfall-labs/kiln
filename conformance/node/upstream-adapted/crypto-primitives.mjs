import crypto, { randomBytes, randomFillSync, getHashes, createHash, createHmac } from 'node:crypto';

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

console.log('pass');
