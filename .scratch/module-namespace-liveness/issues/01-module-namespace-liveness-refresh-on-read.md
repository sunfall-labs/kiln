# Module Namespace Objects Do Not Reflect Live Export Binding Updates

Status: resolved · Severity: medium

## Comments

Fixed 2026-09-03 (+74 Test262 rows).
Proved that refreshing slots on read avoids dereferencing cells across 51 separate object query paths.

## Repro

```js
// mod.mjs
export let counter = 1;
export function bump() { counter += 1; }

// main.mjs
import * as ns from "./mod.mjs";
bump();
console.log(ns.counter); // Expected: 2, Was: 1
```
