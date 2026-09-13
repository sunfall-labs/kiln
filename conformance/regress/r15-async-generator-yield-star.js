// Async yield* must expose the delegated iterator's value, including promise values,
// rather than exposing the iterator-result object to for-await consumers.
const reads = [];
let first = true;
const step = {
  get done() { reads.push('done'); return false; },
  get value() { reads.push('value'); return 42; },
};
const delegate = {
  [Symbol.iterator]() {
    return { next() { if (first) { first = false; return step; } return { done: true, value: 7 }; } };
  },
};
async function* values() { yield* delegate; }

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
  try { for await (const _ of badValues()) {} } catch (error) {
    console.log(error.message, badThrowCalls, badFinally);
  }
}
main();
