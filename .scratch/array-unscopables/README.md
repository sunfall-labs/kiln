# `Array.prototype[Symbol.unscopables]` is missing, and needs a realm-held value

**Status:** open. ~10 test262 rows in `built-ins/Array/prototype/Symbol.unscopables`.

## The gap

```js
Array.prototype[Symbol.unscopables]
// node: an object with a null prototype and 16 keys, every value true:
//   at, copyWithin, entries, fill, find, findIndex, findLast, findLastIndex,
//   flat, flatMap, includes, keys, toReversed, toSorted, toSpliced, values
// kiln: undefined
```

The descriptor node reports is `w0e0c1`. It is what `with (arrayLike) { ... }` consults so that a
method added to `Array.prototype` after a program was written does not capture a variable name the
program already used -- the whole reason the member exists.

## Why it is not a one-line table entry

Every other member of a prototype table is answered by a pure `fn(name) -> Value`:
`array_property`, `symbol_method`, `date_method`. That works because the answer is a `Native`
marker or a string -- values with no identity to preserve.

`@@unscopables` is an **object**, and its identity is observable:

```js
Array.prototype[Symbol.unscopables] === Array.prototype[Symbol.unscopables]  // true
```

A pure function would have to build a fresh one per read and that comparison would be false. So the
object has to be held by the realm, and the realm is not reachable from those functions -- they take
a name and nothing else.

## The shape a fix takes

`Intrinsics` already has exactly the right pattern for this: `error_prototype(kind, realm)` builds
an `ObjectData` on first ask and caches it in a per-realm slot. An `unscopables(realm)` slot beside
it is the same handful of lines.

What is *not* solved by that is reaching it. The value path (`array_property`) is pure; the read
path (`read_property_inner`) has `&mut self` and already intercepts `__proto__` this way, but
`own_descriptor` -- which `getOwnPropertyDescriptor` uses -- is pure too, so intercepting only the
read would make the two disagree, which is the exact class ADR 0054 is about. Both would have to be
routed, or the value would have to be reachable from a pure context.

This is the first concrete case where the virtual-prototype design costs more than the object it is
avoiding, and it is the case ADR 0054's closing paragraph names: materialising the intrinsics as
real objects at realm setup makes this a single ordinary property and needs no interception at all.
Worth doing *with* that change rather than working around it twice.
