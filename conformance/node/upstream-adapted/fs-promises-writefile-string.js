'use strict';
const fs = process.getBuiltinModule('node:fs/promises');
const path = '/tmp/kiln-node-compat-write-string-' + Math.random() + '.tmp';
fs.writeFile(path, 'abc').then(() => fs.readFile(path, 'utf8')).then((value) => {
  if (value !== 'abc') throw new Error('string did not round trip');
  console.log('pass');
});
