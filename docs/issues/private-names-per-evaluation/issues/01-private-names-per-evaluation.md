# Private Names Shared Across Distinct Evaluations of Class Expressions

Status: resolved · Severity: medium

## Comments

Fixed 2026-09-03 (+53 Test262 rows).
Follow-up: `#x in o` private brand query lowering remains open as a sub-case.

## Repro

```js
function factory() {
  return class {
    #m() { return 1; }
    static probe(o) { return o.#m(); }
  };
}
const A = factory(), B = factory();
A.probe(new B()); // Expected: TypeError; Was: 1
```
