// kiln eval smoke test (authored for kiln, not from porffor).
const nacl = require('tweetnacl');

const kp = nacl.sign.keyPair.fromSeed(new Uint8Array(32).fill(7));
const msg = Buffer.from('hello kiln');
const signed = nacl.sign(msg, kp.secretKey);
console.log(nacl.sign.open(signed, kp.publicKey) !== null);
console.log(nacl.sign.detached.verify(msg, nacl.sign.detached(msg, kp.secretKey), kp.publicKey));

const boxKp = nacl.box.keyPair.fromSecretKey(new Uint8Array(32).fill(3));
const nonce = new Uint8Array(24).fill(1);
const boxed = nacl.box(msg, nonce, boxKp.publicKey, boxKp.secretKey);
const opened = nacl.box.open(boxed, nonce, boxKp.publicKey, boxKp.secretKey);
console.log(opened && Buffer.from(opened).toString());

const hash = nacl.hash(new Uint8Array([1, 2, 3]));
console.log(hash.length, Buffer.from(hash).toString('hex').slice(0, 16));
