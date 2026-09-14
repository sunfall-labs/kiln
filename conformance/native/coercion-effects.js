// Coercive operators remain observable even when their results are discarded or loop-invariant.
let calls = 0;
const value = {
  valueOf() {
    calls += 1;
    return 2;
  },
};

for (let i = 0; i < 2; i += 1) {
  value + 1;
  +value;
  value < 10;
}

const throwing = {
  valueOf() {
    calls += 1;
    throw new Error("coercion");
  },
};
let thrown = 0;
try {
  throwing + 1;
} catch (_) {
  thrown += 1;
}

console.log(calls, thrown);
