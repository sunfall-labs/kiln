# `new Date(obj)` Bypasses ToPrimitive in Native Backend

Status: resolved · Severity: medium

## Comments

Fixed 2026-09-06. Verified against interpreter and Node with `conformance/native/date-value-argument.ts`.

## Repro

```ts
let seen = "none";
const spy: any = { [Symbol.toPrimitive]: (h: string) => { seen = h; return 5; } };
const d = new Date(spy);
console.log(seen, d.getTime()); // Node: "default 5"; Was: "none NaN"
```
