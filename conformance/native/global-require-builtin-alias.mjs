import os from 'node:os';
import { createRequire } from 'node:module';
// ESM bundles capture the host's global require before their nested CommonJS wrappers run.
const capturedRequire = createRequire(new URL('./fixtures/unregistered-require-entry.cjs', import.meta.url));
console.log(capturedRequire('os') === os, capturedRequire('node:os') === os);
console.log(capturedRequire('os').platform() === os.platform());
