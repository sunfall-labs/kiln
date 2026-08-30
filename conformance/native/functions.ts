function add(a: number, b: number): number { return a + b; }
function fib(n: number): number { return n < 2 ? n : fib(n - 1) + fib(n - 2); }
const double = (x: number): number => x * 2;
console.log(add(2, 3), double(7), fib(12));

// A closure over a mutable binding: the function and the module body share one box.
let counter = 0;
function bump(): number { counter += 1; return counter; }
console.log(bump(), bump(), counter);

// Two closures over one variable have to observe each other's writes.
function makeNext(start: number) {
  let n = start;
  return function next(): number { n += 1; return n; };
}
const a = makeNext(10);
const b = makeNext(100);
console.log(a(), a(), b(), a());

// A capture threaded one level further in, which is the ParentUpvalue case.
function outer(seed: number) {
  let total = seed;
  function middle() {
    function inner(): number { total += 2; return total; }
    return inner;
  }
  return middle();
}
const deep = outer(5);
console.log(deep(), deep());

// Mutual recursion, and a function passed as a value.
function isEven(n: number): boolean { return n === 0 ? true : isOdd(n - 1); }
function isOdd(n: number): boolean { return n === 0 ? false : isEven(n - 1); }
function apply(f: (x: number) => number, x: number): number { return f(x); }
console.log(isEven(10), isOdd(7), apply(double, 21));

// A missing argument is undefined, and an extra one is discarded.
function arity(x: number, y: number) { return `${x} ${y}`; }
console.log(arity(1), arity(1, 2), arity(1, 2, 3));

// A function is truthy, typeof reports it, and it converts to the same text in both engines.
console.log(typeof add, !add, add === add, add === double, String(double), `${double}`);
