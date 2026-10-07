import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
export const identity = import.meta.url;
export function load() { return require('./value.json'); }
