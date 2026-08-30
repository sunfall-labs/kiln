'use strict';
const fs = process.getBuiltinModule('node:fs/promises');
const path = '/tmp/kiln-node-compat-write-u16-' + Math.random() + '.tmp';
const value = new Uint16Array([0x3130, 0x3332]);
fs.writeFile(path, value).then(() => fs.readFile(path)).then((actual) => {
  if (actual.length !== value.byteLength) throw new Error('Uint16Array byte length mismatch');
  console.log('pass');
});
