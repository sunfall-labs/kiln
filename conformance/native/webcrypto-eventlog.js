// WebCrypto operations used by Effect EventLog encryption and session auth.
const subtle = crypto.subtle;
const hex = (bytes) => Array.from(new Uint8Array(bytes), (b) => b.toString(16).padStart(2, '0')).join('');
const bytes = (hexText) => Uint8Array.from(hexText.match(/../g) || [], (x) => parseInt(x, 16));

const base = await subtle.importKey('raw', bytes('0b'.repeat(22)), 'HKDF', false, ['deriveBits', 'deriveKey']);
const derived = new Uint8Array(await subtle.deriveBits({ name: 'HKDF', hash: 'SHA-256',
  salt: bytes('000102030405060708090a0b0c'), info: bytes('f0f1f2f3f4f5f6f7f8f9') }, base, 336));
if (hex(derived) !== '3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865') {
  throw new Error('HKDF RFC 5869 vector mismatch');
}

const aesKey = await subtle.importKey('raw', bytes('000102030405060708090a0b0c0d0e0f'),
  'AES-GCM', true, ['encrypt', 'decrypt']);
const plain = new TextEncoder().encode('event log payload');
const aad = new TextEncoder().encode('event:42');
const iv1 = bytes('000000000000000000000001'), iv2 = bytes('000000000000000000000002');
const encrypted1 = await subtle.encrypt({ name: 'AES-GCM', iv: iv1, additionalData: aad }, aesKey, plain);
const encrypted2 = await subtle.encrypt({ name: 'AES-GCM', iv: iv2, additionalData: aad }, aesKey, plain);
if (hex(encrypted1) === hex(encrypted2)) throw new Error('AES-GCM IV did not affect ciphertext');
if (new TextDecoder().decode(await subtle.decrypt({ name: 'AES-GCM', iv: iv1, additionalData: aad }, aesKey, encrypted1)) !== 'event log payload') {
  throw new Error('AES-GCM round trip mismatch');
}
let authFailed = false;
try { await subtle.decrypt({ name: 'AES-GCM', iv: iv1, additionalData: bytes('00') }, aesKey, encrypted1); }
catch (error) { authFailed = error.name === 'OperationError'; }
if (!authFailed) throw new Error('AES-GCM accepted modified additional data');

const seed = bytes('9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60');
const pkcs8 = bytes('302e020100300506032b657004220420' + hex(seed));
const privateKey = await subtle.importKey('pkcs8', pkcs8, 'Ed25519', true, ['sign']);
const publicKey = await subtle.importKey('raw', bytes('d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a'),
  'Ed25519', true, ['verify']);
const signature = await subtle.sign('Ed25519', privateKey, new Uint8Array(0));
if (hex(signature) !== 'e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155' +
  '5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b') throw new Error('Ed25519 RFC 8032 vector mismatch');
if (!await subtle.verify('Ed25519', publicKey, signature, new Uint8Array(0))) throw new Error('Ed25519 verify failed');
if (await subtle.verify('Ed25519', publicKey, new Uint8Array(1), new Uint8Array(0))) throw new Error('Ed25519 accepted malformed signature');
const exportedJwk = await subtle.exportKey('jwk', privateKey);
if (exportedJwk.x !== '11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo' || !exportedJwk.d) throw new Error('Ed25519 JWK export mismatch');
const generated = await subtle.generateKey('Ed25519', true, ['sign', 'verify']);
const generatedPublic = await subtle.exportKey('raw', generated.publicKey);
const generatedSignature = await subtle.sign('Ed25519', generated.privateKey, plain);
if (!await subtle.verify('Ed25519', generated.publicKey, generatedSignature, plain) || generatedPublic.byteLength !== 32) {
  throw new Error('generated Ed25519 pair did not verify');
}

const hmacKey = await subtle.importKey('raw', bytes('0b'.repeat(20)),
  { name: 'HMAC', hash: 'SHA-256' }, false, ['sign', 'verify']);
const hmac = await subtle.sign('HMAC', hmacKey, new TextEncoder().encode('Hi There'));
if (hex(hmac) !== 'b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7' ||
    !await subtle.verify('HMAC', hmacKey, hmac, new TextEncoder().encode('Hi There')) ||
    await subtle.verify('HMAC', hmacKey, new Uint8Array(1), new TextEncoder().encode('Hi There'))) {
  throw new Error('HMAC-SHA256 vector/verification mismatch');
}

const savedRandom = Math.random;
Math.random = () => { throw new Error('Math.random must not supply cryptographic bytes'); };
const backing = new Uint8Array([0xaa, 0, 0, 0xbb]), view = backing.subarray(1, 3);
if (crypto.getRandomValues(view) !== view || backing[0] !== 0xaa || backing[3] !== 0xbb) {
  throw new Error('getRandomValues lost its view or wrote outside its byte range');
}
Math.random = savedRandom;
let quotaError = false;
try { crypto.getRandomValues(new Uint8Array(65537)); }
catch (error) { quotaError = error.name === 'QuotaExceededError'; }
if (!quotaError) throw new Error('getRandomValues did not enforce its 65,536-byte quota');
let typeError = false;
try { crypto.getRandomValues(new DataView(new ArrayBuffer(1))); }
catch (error) { typeError = error.name === 'TypeMismatchError'; }
if (!typeError) throw new Error('getRandomValues accepted a DataView');
const floatView = new Float32Array(1);
Object.defineProperty(floatView, Symbol.toStringTag, { value: 'Uint8Array' });
typeError = false;
try { crypto.getRandomValues(floatView); }
catch (error) { typeError = error.name === 'TypeMismatchError'; }
if (!typeError) throw new Error('getRandomValues trusted a spoofed floating-view tag');
const integerView = new Uint8Array(1);
Object.defineProperty(integerView, Symbol.toStringTag, { value: 'Float32Array' });
if (crypto.getRandomValues(integerView) !== integerView) throw new Error('getRandomValues rejected an integer view with a spoofed tag');

console.log('webcrypto eventlog vectors passed');
