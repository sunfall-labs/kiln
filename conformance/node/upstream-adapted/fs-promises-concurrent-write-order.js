'use strict';
const fs = process.getBuiltinModule('node:fs/promises');
const first = '/tmp/kiln-node-compat-concurrent-a-' + Math.random() + '.tmp';
const second = '/tmp/kiln-node-compat-concurrent-b-' + Math.random() + '.tmp';
const seen = [];
fs.writeFile(first, 'a').then(() => seen.push('a'));
fs.writeFile(second, 'b').then(() => seen.push('b'));
Promise.resolve().then(() => seen.push('microtask'));
setTimeout(() => {
  if (seen[0] !== 'microtask' || seen.length !== 3 || !seen.includes('a') || !seen.includes('b')) {
    throw new Error('concurrent completion order mismatch');
  }
  console.log('pass');
}, 0);
