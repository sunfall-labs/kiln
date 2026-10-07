// Exactly the shape of conformance/node/upstream-adapted/fs-promises-concurrent-write-order.js,
// but reporting what happened instead of throwing.
import fs from 'node:fs/promises';
const first = '/tmp/kiln-probe-a-' + Math.random() + '.tmp';
const second = '/tmp/kiln-probe-b-' + Math.random() + '.tmp';
const seen = [];
const a = fs.writeFile(first, 'a').then(() => seen.push('a'));
const b = fs.writeFile(second, 'b').then(() => seen.push('b'));
Promise.resolve().then(() => seen.push('microtask'));
setTimeout(() => {
  console.log('at-timeout=' + JSON.stringify(seen));
  Promise.all([a, b]).then(() => {
    console.log('at-settle=' + JSON.stringify(seen));
  });
}, 0);
