'use strict';
// The read half of the concurrency pair; see `fs-promises-concurrent-write-order.js` for why the
// oracle is the settlement of the reads rather than a `setTimeout(0)`. Measured the same way on
// node v22.23.2: of 100 runs, the timer saw both reads 30 times, neither 63 times, and one of the
// two 7 times.
const fs = process.getBuiltinModule('node:fs/promises');
const seen = [];
const readText = fs
  .readFile('conformance/node/fixtures/text.txt', 'utf8')
  .then((value) => seen.push(value.length));
const readEmpty = fs
  .readFile('conformance/node/fixtures/empty.txt')
  .then((value) => seen.push(value.length));
Promise.resolve().then(() => seen.push('microtask'));
Promise.all([readText, readEmpty]).then(() => {
  if (seen[0] !== 'microtask') {
    throw new Error('a filesystem completion ran before the microtask checkpoint');
  }
  if (seen.length !== 3) throw new Error('checkpoint order mismatch');
  // The two reads differ in size, so their lengths identify them whichever order they landed in.
  if (!seen.includes(12) || !seen.includes(0)) {
    throw new Error('concurrent reads did not both deliver their bytes');
  }
  setTimeout(() => {
    if (seen.length !== 3) {
      throw new Error('a filesystem completion ran after a timer armed from a completion');
    }
    console.log('pass');
  }, 0);
});
