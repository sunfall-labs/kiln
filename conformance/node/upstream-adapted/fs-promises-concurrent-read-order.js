'use strict';
const fs = process.getBuiltinModule('node:fs/promises');
const seen = [];
fs.readFile('conformance/node/fixtures/text.txt', 'utf8').then((value) => seen.push(value.length));
fs.readFile('conformance/node/fixtures/empty.txt').then((value) => seen.push(value.length));
Promise.resolve().then(() => seen.push('microtask'));
setTimeout(() => {
  if (seen[0] !== 'microtask' || seen.length !== 3) throw new Error('checkpoint order mismatch');
  console.log('pass');
}, 0);
