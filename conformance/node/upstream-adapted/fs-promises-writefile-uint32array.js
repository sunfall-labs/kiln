'use strict';
const fs = process.getBuiltinModule('node:fs/promises');
const path = '/tmp/kiln-node-compat-write-u32-' + Math.random() + '.tmp';
const value = new Uint32Array([0x33323130]);
fs.writeFile(path, value).then(() => fs.readFile(path)).then((actual) => {
  if (actual.length !== value.byteLength) throw new Error('Uint32Array byte length mismatch');
  console.log('pass');
});
