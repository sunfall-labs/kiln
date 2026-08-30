'use strict';
const fs = process.getBuiltinModule('node:fs/promises');
fs.readFile('conformance/node/fixtures/text.txt').then((value) => {
  if (!(value instanceof Uint8Array)) throw new Error('readFile did not return bytes');
  if (value.length !== 12 || value[0] !== 104 || value[11] !== 10) throw new Error('file bytes mismatch');
  console.log('pass');
});
