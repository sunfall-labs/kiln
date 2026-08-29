// Closures capture the binding, not the value: two closures over one variable observe each
// other's writes, and the binding outlives the call that created it.
function makeCounter() {
  let count = 0;
  function increment(): number {
    count += 1;
    return count;
  }
  return increment;
}

const first = makeCounter();
const second = makeCounter();

console.log("first", first(), first(), first());
console.log("second", second());
console.log("independent", first());

// A parameter is captured the same way a local is.
function adder(n: number) {
  function add(m: number): number {
    return n + m;
  }
  return add;
}
const addTen = adder(10);
console.log("adder", addTen(5), addTen(1));

// Captures thread through every function in between, one hop at a time.
function outer(x: number): number {
  function middle(): number {
    function inner(): number {
      return x;
    }
    return inner();
  }
  return middle();
}
console.log("nested", outer(42));
