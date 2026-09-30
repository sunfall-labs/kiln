// kiln eval smoke test (authored for kiln, not from porffor).
import { sha256, sha512 } from '@noble/hashes/sha2.js';
import { sha3_256, keccak_256 } from '@noble/hashes/sha3.js';
import { blake2b } from '@noble/hashes/blake2.js';
import { hmac } from '@noble/hashes/hmac.js';
import { utf8ToBytes } from '@noble/hashes/utils.js';

const hex = (b) => Buffer.from(b).toString('hex');
const abc = utf8ToBytes('abc');
console.log('sha256(abc):', hex(sha256(abc)));
console.log('sha512(abc)[:16]:', hex(sha512(abc)).slice(0, 32));
console.log('sha3_256(abc):', hex(sha3_256(abc)));
console.log('keccak_256(abc):', hex(keccak_256(abc)));
console.log('blake2b(abc):', hex(blake2b(abc)));
console.log('hmac-sha256:', hex(hmac(sha256, utf8ToBytes('key'), abc)));
const h = sha256.create();
h.update(utf8ToBytes('a'));
h.update(utf8ToBytes('bc'));
console.log('incremental === one-shot:', hex(h.digest()) === hex(sha256(abc)));
console.log('outputLen:', sha256.outputLen, sha512.outputLen);
