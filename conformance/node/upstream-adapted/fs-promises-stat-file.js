'use strict';
const fs = process.getBuiltinModule('node:fs/promises');
fs.stat('conformance/node/fixtures/text.txt').then((stat) => {
  if (!stat.isFile() || stat.isDirectory() || stat.size !== 12) throw new Error('file stat mismatch');
  console.log('pass');
});
