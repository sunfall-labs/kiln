// kiln eval smoke test (authored for kiln, not from porffor).
import { secp256k1 } from '@noble/curves/secp256k1.js';
import { ed25519 } from '@noble/curves/ed25519.js';
import { p256 } from '@noble/curves/nist.js';

const hex = (b) => Buffer.from(b).toString('hex');
const priv = new Uint8Array(32).fill(1);
const pub = secp256k1.getPublicKey(priv);
console.log('secp256k1 pub len:', pub.length, 'prefix:', hex(pub).slice(0, 4));
const msg = new Uint8Array([1, 2, 3]);
const sig = secp256k1.sign(msg, priv);
console.log('secp256k1 verify:', secp256k1.verify(sig, msg, pub));
console.log('secp256k1 verify tampered:', secp256k1.verify(sig, new Uint8Array([9, 9, 9]), pub) === false);

const edPriv = ed25519.utils.randomSecretKey().fill(7);
const edPub = ed25519.getPublicKey(edPriv);
const edSig = ed25519.sign(msg, edPriv);
console.log('ed25519 verify:', ed25519.verify(edSig, msg, edPub));

const p256pub = p256.getPublicKey(priv);
console.log('p256 pub len:', p256pub.length);
console.log('secp256k1 shared-ish:', hex(secp256k1.getSharedSecret(priv, pub).slice(1, 9)));
