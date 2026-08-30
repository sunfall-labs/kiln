'use strict';
const fs = process.getBuiltinModule('node:fs/promises');
fs.readFile('conformance/node/fixtures/empty.txt').then((value) => {
  if (!(value instanceof Uint8Array)) throw new Error('readFile did not return bytes');
  if (value.length !== 0) throw new Error('empty file was not empty');
  console.log('pass');
});
