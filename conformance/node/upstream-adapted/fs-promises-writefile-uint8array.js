'use strict';
const fs = process.getBuiltinModule('node:fs/promises');
const path = '/tmp/kiln-node-compat-write-u8-' + Math.random() + '.tmp';
const value = new Uint8Array([48, 49, 50]);
fs.writeFile(path, value).then(() => fs.readFile(path)).then((actual) => {
  if (actual.length !== 3 || actual[0] !== 48 || actual[2] !== 50) throw new Error('Uint8Array mismatch');
  console.log('pass');
});
