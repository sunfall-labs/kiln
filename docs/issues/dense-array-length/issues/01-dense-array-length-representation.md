# Array Length Cannot Exceed Dense Storage Capacity

Status: wontfix · Severity: low

## Comments

Deliberately not attempted: 133 Test262 rows (e.g. `built-ins/Array` high-index edge cases). Real code does not allocate 4-billion-element arrays, and decoupling length from `Vec<Value>` degrades primary array fast paths across both backends.

## Symptoms

```js
new Array(2**32 - 2); // throws RangeError / dense representation refusal
```
