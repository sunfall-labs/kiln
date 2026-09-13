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
async function checkValue() {
  const out = [];
  for await (const value of values()) out.push(value);
  console.log(out.join(','), reads.join(','));
}
let finallyRan = false;
const bad = { [Symbol.iterator]() { return { next() { return { done: false, get value() { throw new Error('value'); } }; } }; } };
async function* throwsFromValue() { try { yield* bad; } finally { finallyRan = true; } }
async function checkThrow() {
  try { for await (const _ of throwsFromValue()) {} } catch (error) { console.log(error.message, finallyRan); }
}
checkValue().then(checkThrow);
