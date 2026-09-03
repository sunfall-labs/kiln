'use strict';
// Two `fs.promises` writes in flight at once. What is ordered here is the *promise job* order,
// not the wall clock: the microtask checkpoint that follows the script drains before any
// filesystem completion is delivered, and a timer armed from inside a completion callback is a
// later turn than that callback.
//
// This case used to make `setTimeout(0)` its oracle and require both writes to have landed by
// then. That is a race, and Node loses it more often than it wins: on node v22.23.2, 100 runs of
// the same program saw both writes at the timer 35 times, neither 58 times, and one of the two
// 7 times -- 8 of 40 under CPU load. A due timer does not wait for in-flight threadpool work in
// either engine, by design (see `RealmRuntime::next_turn`), so the ordering is anchored on the
// settlement of the writes, where it is defined.
const fs = process.getBuiltinModule('node:fs/promises');
const first = '/tmp/kiln-node-compat-concurrent-a-' + Math.random() + '.tmp';
const second = '/tmp/kiln-node-compat-concurrent-b-' + Math.random() + '.tmp';
const seen = [];
const wroteFirst = fs.writeFile(first, 'a').then(() => seen.push('a'));
const wroteSecond = fs.writeFile(second, 'b').then(() => seen.push('b'));
Promise.resolve().then(() => seen.push('microtask'));
Promise.all([wroteFirst, wroteSecond]).then(() => {
  if (seen[0] !== 'microtask') {
    throw new Error('a filesystem completion ran before the microtask checkpoint');
  }
  if (seen.length !== 3 || !seen.includes('a') || !seen.includes('b')) {
    throw new Error('concurrent completion order mismatch');
  }
  // Both writes resolved concurrently; both sets of bytes must be on disk, not just one.
  return Promise.all([fs.readFile(first, 'utf8'), fs.readFile(second, 'utf8')]);
}).then((contents) => {
  if (contents[0] !== 'a' || contents[1] !== 'b') {
    throw new Error('concurrent writes did not both reach disk');
  }
  setTimeout(() => {
    if (seen.length !== 3) {
      throw new Error('a filesystem completion ran after a timer armed from a completion');
    }
    console.log('pass');
  }, 0);
});
