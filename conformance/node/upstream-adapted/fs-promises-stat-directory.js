'use strict';
const fs = process.getBuiltinModule('node:fs/promises');
fs.stat('conformance/node/fixtures').then((stat) => {
  if (stat.isFile() || !stat.isDirectory()) throw new Error('directory stat mismatch');
  console.log('pass');
});
