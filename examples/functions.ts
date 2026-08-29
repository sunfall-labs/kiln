// Functions, calls and module scope: the slice that makes everything above it reachable.
function add(a: number, b: number): number {
  return a + b;
}

// A function can call itself: the binding exists before its body is compiled.
function factorial(n: number): number {
  if (n <= 1) {
    return 1;
  }
  return n * factorial(n - 1);
}

// Module scope is shared by every call rather than copied into each frame, so a write from
// inside one call is visible to the next.
let calls = 0;
function counted(value: number): number {
  calls += 1;
  return value;
}

console.log("add", add(2, 3));
console.log("factorial", factorial(5));
console.log("counted", counted(1), counted(2), counted(3));
console.log("calls", calls);
