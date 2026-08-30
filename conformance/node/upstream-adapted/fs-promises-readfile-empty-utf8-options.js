'use strict';
const fs = process.getBuiltinModule('node:fs/promises');
fs.readFile('conformance/node/fixtures/empty.txt', { encoding: 'utf8' }).then((value) => {
  if (value !== '') throw new Error('options encoding was not honored');
  console.log('pass');
});
