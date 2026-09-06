# A method call site remembers a prototype method by value with nothing to forget it

Found 2026-09-06 while reading `kiln_c99_val_call_method`'s cache fill for the typed method
entries. A site that resolved `dist` on `Point.prototype` remembers the closure *by value* in
the entry for the receiver's shape (an own property is remembered by slot, which is right). The
receiver's shape does not change when `Point.prototype.dist` is reassigned, so the site keeps
calling the closure it first saw.

```js
class Point { constructor(x) { this.x = x; } dist() { return this.x; } }
const p = new Point(3);
function call() { return p.dist(); }
console.log(call());
Point.prototype.dist = function () { return -this.x; };
console.log(call());
```

Expected `3` then `-3`. Whether the native backend prints `3` twice depends on the site having
been filled before the reassignment; the emitted by-name and typed dispatch inherit the cached
value, so they are exactly as right as the cache.

Triage: bug, native backend, soundness. The fix is an invalidation: a write to a property of
an object that is some shape's prototype bumps an epoch the cached entries are stamped with, or
the cache remembers the prototype's shape as well and a prototype write transitions it. The
plan's row 2.4 said epochs come only if open-world programs need them; this is the case that
does.
