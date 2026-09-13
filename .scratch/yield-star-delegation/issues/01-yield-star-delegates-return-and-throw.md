# `yield*` Does Not Delegate `return` or `throw` to Inner Iterator

Status: resolved · Severity: high

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

## Comments

Resolved:
- Implemented full ECMA-262 §14.4.14 iterator delegation protocol across `kiln-ir`, `kiln-interpreter`, and `kiln-backend-c`.
- `yield*` delegates `.return()` and `.throw()` completions to the inner iterator's `return` and `throw` methods.
- If inner iterator lacks `throw`, its `return` method is called (if present) and a `TypeError` is thrown.
- Outer generator yields inner iterator's `{ value, done }` steps directly without wrapping or re-reading `.value` when `!done`.
- Added `Instruction::JumpIfReturnSignal` to cleanly bypass `catch` blocks when unwinding a generator via `return()`.
- Test262 `language/expressions/yield` pass count reached 122/123 (0 yield* failures remaining; remaining 1 failure is pre-existing unrelated parameter mapping).
- Native parity verified with `scripts/native.sh`: 160 matched, 0 diverged.
