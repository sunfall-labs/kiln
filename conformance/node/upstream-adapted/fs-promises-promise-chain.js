'use strict';
const fs = process.getBuiltinModule('node:fs/promises');
const seen = [];
fs.readFile('conformance/node/fixtures/text.txt', 'utf8')
  .then((value) => {
    seen.push(value.length);
    return fs.stat('conformance/node/fixtures/text.txt');
  })
  .then((stat) => {
    seen.push(stat.size);
    return fs.readFile('conformance/node/fixtures/empty.txt');
  })
  .then((empty) => {
    if (seen[0] !== 12 || seen[1] !== 12 || empty.length !== 0) throw new Error('promise chain mismatch');
    console.log('pass');
  });
