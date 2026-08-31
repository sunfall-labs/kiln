import data from './json-module-data.json';
import required from './json-module-commonjs.cjs';

if (data.name !== 'kiln' || data.nested.port !== 3000) throw new Error('ESM JSON');
if (required !== data) throw new Error('JSON module cache identity');
console.log('pass');
