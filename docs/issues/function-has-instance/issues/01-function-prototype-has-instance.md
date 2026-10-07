# Function.prototype[Symbol.hasInstance] Missing Property Descriptor

Status: resolved · Severity: medium

## Comments

Fixed 2026-09-03 (+14 rows in `built-ins/Function`).
Key architectural learning: Avoid threading operator-specific state into shared prototype evaluation; intrinsics answer `error_prototype(kind)` directly from the realm.

## Symptoms

```js
Object.getOwnPropertyDescriptor(Function.prototype, Symbol.hasInstance);
// Expected: { writable: false, enumerable: false, configurable: false, value: [Function: [Symbol.hasInstance]] }
```
