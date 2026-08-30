'use strict';
const fs = process.getBuiltinModule('node:fs/promises');
fs.readFile('conformance/node/fixtures/empty.txt', 'utf8').then((value) => {
  if (value !== '') throw new Error('empty UTF-8 file was not an empty string');
  console.log('pass');
});
