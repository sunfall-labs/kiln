'use strict';
const fs = process.getBuiltinModule('node:fs/promises');
fs.stat('conformance/node/fixtures/does-not-exist').then(
  () => { throw new Error('missing path unexpectedly existed'); },
  (error) => {
    if (error.code !== 'ENOENT' || error.syscall !== 'stat') throw new Error('wrong stat error shape');
    if (error.path !== 'conformance/node/fixtures/does-not-exist') throw new Error('wrong stat error path');
    console.log('pass');
  }
);
