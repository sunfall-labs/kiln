import os from 'node:os';
// ESM bundles capture the host's global require before their nested CommonJS wrappers run.
const capturedRequire = globalThis.require;
console.log(capturedRequire('os') === os, capturedRequire('node:os') === os);
console.log(capturedRequire('os').platform() === os.platform());
