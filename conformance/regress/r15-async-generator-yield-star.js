// Async yield* must expose the delegated iterator's value, including promise values,
// rather than exposing the iterator-result object to for-await consumers.
const reads = [];
const step = {
  get done() { reads.push('done'); return false; },
  get value() { reads.push('value'); return 42; },
};
const delegate = {
  [Symbol.iterator]() {
    let index = 0;
    return { next() {
      if (index++ === 0) return step;
      if (index === 2) return { done: false, value: Promise.resolve(9) };
      return { done: true, value: 7 };
    } };
  },
};
async function* values() { yield* delegate; return 8; }

let badFinally = false;
let badThrowCalls = 0;
const badDelegate = {
  [Symbol.iterator]() {
    return {
      next() { return { done: false, get value() { throw new Error('getter boom'); } }; },
      throw(reason) { badThrowCalls++; return { done: true, value: 'delegate handled' }; },
    };
  },
};
async function* badValues() { try { yield* badDelegate; } finally { badFinally = true; } }

async function main() {
  const out = [];
  for await (const value of values()) out.push(value);
  console.log(out.join(','), reads.join(','));
  reads.length = 0;
  const completion = values();
  await completion.next();
  await completion.next();
  const finished = await completion.next();
  console.log(finished.value, finished.done, reads.join(','));
  try { for await (const _ of badValues()) {} } catch (error) {
    console.log(error.message, badThrowCalls, badFinally);
  }
}
main();
