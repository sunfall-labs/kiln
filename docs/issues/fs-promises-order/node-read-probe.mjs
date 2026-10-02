// The shape of conformance/node/upstream-adapted/fs-promises-concurrent-read-order.js.
import fs from 'node:fs/promises';
const seen = [];
const a = fs
  .readFile('conformance/node/fixtures/text.txt', 'utf8')
  .then((value) => seen.push('text:' + value.length));
const b = fs
  .readFile('conformance/node/fixtures/empty.txt')
  .then((value) => seen.push('empty:' + value.length));
Promise.resolve().then(() => seen.push('microtask'));
setTimeout(() => {
  console.log('at-timeout=' + JSON.stringify(seen));
  Promise.all([a, b]).then(() => {
    console.log('at-settle=' + JSON.stringify(seen));
  });
}, 0);
