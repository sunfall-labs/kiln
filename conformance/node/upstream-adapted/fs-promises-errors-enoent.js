'use strict';
const fs = process.getBuiltinModule('node:fs/promises');
fs.readFile('conformance/node/fixtures/does-not-exist').then(
  () => { throw new Error('missing file unexpectedly existed'); },
  (error) => {
    if (error.code !== 'ENOENT' || error.syscall !== 'open') throw new Error('wrong Node error shape');
    console.log('pass');
  }
);
