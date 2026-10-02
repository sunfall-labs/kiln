// kiln-native: profile=native-release
const moduleUrl = import.meta.url;
const getBuiltinModule = process.getBuiltinModule;

// The public global is writable. Compiler-generated import.meta and builtin facades must retain
// access to the realm's host capabilities after that public property changes.
globalThis.process = null;

const pathViaGetBuiltin = getBuiltinModule('node:path');
const nodeModule = await import('node:module');
const nodeCrypto = await import('node:crypto');
const require = nodeModule.createRequire(moduleUrl);
const path = require('node:path');
const digest = nodeCrypto.createHash('sha256').update('capability').digest('hex');
let invalidKeyRejectedByCrypto = false;
try {
  nodeCrypto.createPrivateKey('not a private key');
} catch (error) {
  invalidKeyRejectedByCrypto = error instanceof Error && !(error instanceof TypeError);
}

if (!moduleUrl.endsWith('private-capabilities-survive-process-replacement.mjs') ||
    typeof path.basename !== 'function' || typeof pathViaGetBuiltin.basename !== 'function' ||
    digest.length !== 64 || !invalidKeyRejectedByCrypto) {
  throw new Error('private realm capabilities followed the mutable process global');
}
console.log('private realm capabilities survive process replacement');
