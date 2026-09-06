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

## Addendum 2026-09-06: the shape does not carry the prototype

Probed while judging a shape-keyed dispatch. Instances built by two classes whose constructors
store the same keys in the same order are told apart correctly at one call site (`new Point`
and `new Other` alternating, and a literal `{x, y}` re-prototyped to `Point.prototype`
alternating with `new Other`): whatever a constructed instance's shape is, it differs per
class. But an instance whose prototype is swapped *after* construction keeps its shape, and the
site then calls the old class's method:

```js
class Point { constructor(x, y) { this.x = x; this.y = y; } dist() { return this.x + this.y; } }
class Other { constructor(x, y) { this.x = x; this.y = y; } dist() { return this.x * this.y; } }
const p = new Point(3, 4), q = new Point(5, 6);
Object.setPrototypeOf(q, Other.prototype);
for (const it of [p, q, p, q]) console.log(it.dist());   // node: 7,30,7,30  native: 7,11,7,11
```

Same root cause as above: the cache remembers a prototype method by value under the receiver's
shape, and nothing about the shape changes when the prototype does. Either `setPrototypeOf`
transitions the shape (V8 gives the object a fresh map), or the entry also remembers the
prototype it resolved through and the hit compares it. The second is one load and one compare
on the hit path and needs no shape-tree change.
