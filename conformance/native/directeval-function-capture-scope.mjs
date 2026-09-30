function factory() {
  let count = 0;
  const fixed = 'constant';
  const next = eval('(function (count) { "use strict"; count += 1; return [count, fixed, this === undefined]; })');
  const increment = eval('(function () { count += 1; return count; })');
  const assignConst = eval('(function () { fixed = "changed"; })');
  return { next, increment, assignConst, read: () => count };
}

const result = factory();
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
