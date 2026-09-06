# `yield*` Does Not Delegate `return` or `throw` to Inner Iterator

Status: ready-for-agent · Severity: high

## Repro

```js
const inner = {
  [Symbol.iterator]() {
    return {
      next() { return { value: 1, done: false }; },
      return() { return 5; }, // Non-object result
    };
  }
};
function* g() { yield* inner; }
const it = g();
it.next();
it.return(42);
// Node: TypeError (inner return() did not return an object)
// Kiln: { value: 42, done: true } (inner return() ignored)
```
