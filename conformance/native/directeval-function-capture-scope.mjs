function factory(sourceNext, sourceIncrement, sourceConst) {
  let count = 0;
  const fixed = 'constant';
  const next = eval(sourceNext);
  const increment = eval(sourceIncrement);
  const assignConst = eval(sourceConst);
  return { next, increment, assignConst, read: () => count };
}

const result = factory(
  '(function (count) { "use strict"; count += 1; return [count, fixed, this === undefined]; })',
  '(function () { count += 1; return count; })',
  '(function () { fixed = "changed"; })',
);
console.log(result.next(10).join(','));
console.log(result.increment(), result.increment(), result.read());
try {
  result.assignConst();
} catch (error) {
  console.log(error instanceof TypeError);
}

const intrinsicEval = eval;
const order = [];
Object.defineProperty(globalThis, 'eval', {
  configurable: true,
  get() {
    order.push('callee');
    return intrinsicEval;
  },
});
function ordered() {
  return eval((order.push('argument'), '(function () { return 7; })'))();
}
console.log(ordered(), order.join(','));
