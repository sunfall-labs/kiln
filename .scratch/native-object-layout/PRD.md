# PRD: Native Object Layout Field Alignment with Private Members

Status: needs-triage
Branch: `investigate/native-object-layout`

## Problem Statement

In C native backend, a constructor that calls a static private method loses the instance's private fields, triggering memory corruption or null slot reads.

## Symptoms

```js
class A {
  static #helper(inst) { inst.#field = 123; }
  #field = 1;
  constructor() {
    A.#helper(this);
  }
}
new A();
```
