import { createRequire } from 'node:module';
globalThis.require = createRequire(new URL('./source/packages/effect/package.json', import.meta.url));
