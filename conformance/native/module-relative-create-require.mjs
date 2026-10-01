// kiln-native: profile=native-release
import { createRequire } from 'node:module';
import { load as left, identity as leftIdentity } from './support/require-left/module.mjs';
import { load as right, identity as rightIdentity } from './support/require-right/module.mjs';
const require = createRequire(import.meta.url);
const entry = require('./support/require-entry.json');
if (entry.name !== 'entry' || left().name !== 'left' || right().name !== 'right' ||
    left() !== left() || right() !== right() || leftIdentity === rightIdentity) {
  throw new Error('bundled modules lost their relative require base or cache identity');
}
console.log(entry.name, left().name, right().name, leftIdentity !== rightIdentity);
