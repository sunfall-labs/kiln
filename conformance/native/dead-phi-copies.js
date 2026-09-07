// Live loop-carried parallel copies must survive pruning of unrelated merge chains.
function rotate(n) {
  let a = 1, b = 2, c = 3;
  let unused = 9;
  for (let i = 0; i < n; i++) {
    const old = a;
    a = b; b = c; c = old;
    if (i % 2) unused = 10; else unused = 11;
  }
  return a * 100 + b * 10 + c;
}
console.log(rotate(0), rotate(1), rotate(2), rotate(3), rotate(10));

// Discarded values can still invoke getters/coercions and throw.
let effects = 0;
const object = {
  get value() { effects++; return 7; },
  valueOf() { effects++; throw 'coercion'; },
};
function effectful(flag) {
  let unused;
  if (flag) unused = object.value; else unused = 8;
  try { unused = object + 1; } catch (error) { console.log(error); }
  finally { effects += 10; }
  return effects;
}
console.log(effectful(true));

// Captured bindings must remain visible across loop backedges and exception edges.
function captured() {
  let value = 0;
  const read = () => value;
  for (let i = 0; i < 3; i++) value += i;
  try { value = 9; throw 1; } catch (_) { console.log(read()); }
  return read();
}
console.log(captured());

// The stack below yield/await is restored positionally, including discarded results.
function* suspended() {
  100 + (yield 1);
  return 200 + (yield 2);
}
const sequence = suspended();
console.log(sequence.next().value, sequence.next(7).value, sequence.next(8).value);
async function delayed() {
  100 + await Promise.resolve(7);
  return 200 + await Promise.resolve(8);
}
delayed().then(value => console.log(value));
