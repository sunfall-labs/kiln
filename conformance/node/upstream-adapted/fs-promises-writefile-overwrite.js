'use strict';
const fs = process.getBuiltinModule('node:fs/promises');
const path = '/tmp/kiln-node-compat-overwrite-' + Math.random() + '.tmp';
fs.writeFile(path, 'longer value')
  .then(() => fs.writeFile(path, 'x'))
  .then(() => fs.readFile(path, 'utf8'))
  .then((value) => {
    if (value !== 'x') throw new Error('writeFile did not replace and truncate');
    console.log('pass');
  });
