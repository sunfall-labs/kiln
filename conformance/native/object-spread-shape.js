// `{ ...source }` when both the source and the freshly-built target are shaped objects: the fast
// path that copies a shape's slots in one `memcpy` instead of walking `kiln_c99_put` once per key
// (interning the name again, transitioning the shape one key at a time, and for a string value
// allocating a fresh copy through the boundary conversion). That path only fires when the target
// is still the shape's empty starting point and the source has no elements, no accessors and no
// symbol keys among the slots being bulk-copied -- every other shape is intentionally still
// exercised here so a shape that should keep the slow path does.
//
// Checked against node directly, the way object-spread-sources.js is: this corpus otherwise uses
// the interpreter as its oracle, and a wrong interpreter would happily agree with a wrong native
// backend.

const base = { a: 1, b: 'hello', c: true, d: [1, 2], e: 42 };

// The plain case the fast path exists for: a fresh literal spreading a fresh shaped source with
// keys added after.
const merged = { ...base, f: 7, g: 14 };
console.log('values          ', JSON.stringify(merged));
console.log('keys            ', JSON.stringify(Object.keys(merged)));

// A spread copies references, not clones: the array slot in the copy must be the *same* array,
// and mutating a scalar slot in the copy must not touch the source.
console.log('array identity  ', merged.d === base.d);
merged.a = 999;
console.log('source untouched', base.a, merged.a);

// Types that pass through a slot unlike a string do (numbers, booleans, undefined, null, and a
// nested object) are worth checking on their own terms rather than trusting the string case to
// stand for all of them.
const typed = { num: 1, bool: false, undef: undefined, nul: null, obj: { nested: true } };
const typedMerged = { ...typed };
console.log('typed values    ', JSON.stringify(typedMerged));
console.log('typed undef key ', 'undef' in typedMerged, typedMerged.undef);
console.log('typed obj ident ', typedMerged.obj === typed.obj);

// Key order: the source's own order, then whatever the literal added after it.
console.log('key order       ', JSON.stringify(Object.keys({ ...base, f: 1, g: 2 })));

// Spreading into a literal that already has keys -- the target is not the shape's empty starting
// point, so this must take the slow, per-key path rather than the fast one.
console.log('preceded        ', JSON.stringify({ x: 1, ...base }));
console.log('preceded keys   ', JSON.stringify(Object.keys({ x: 1, ...base })));

// A key spread from the source and then overridden by a later literal key: the override must win,
// and must not corrupt any other copied slot.
console.log('overridden      ', JSON.stringify({ ...base, a: 9 }));

// A symbol key on the source. Symbol-keyed spreads need the existing per-key handling (a shaped
// object's slots may include one, spelled "@@..." in the shape's keys), so this must still work
// -- and still carry the symbol across, which `Object.keys` does not see but a re-read by the
// same symbol does.
const sym = Symbol('tag');
const withSymbol = { h: 1, [sym]: 'symbolic' };
const symSpread = { ...withSymbol };
console.log('symbol carried  ', symSpread[sym], Object.keys(symSpread).length);

// An integer key on the source. Elements are handled by a separate loop from the shape's named
// slots, and a source that has any must not take the bulk-slot fast path.
const withIndex = { 0: 'z', a: 1 };
console.log('integer key     ', JSON.stringify({ ...withIndex }));
console.log('integer keys    ', JSON.stringify(Object.keys({ ...withIndex })));

// A getter on the source: spreading must call it and copy what it returned, not the accessor
// itself. A shaped object can never hold an accessor (defining one converts the object to
// dictionary mode), so this source is necessarily in dictionary mode already and exercises the
// pre-existing per-property path, not the new one -- included so the boundary between the two
// stays correct.
let getterCalls = 0;
const withGetter = {
  get g() {
    getterCalls += 1;
    return 1;
  },
};
const getterSpread = { ...withGetter };
console.log('getter value    ', getterSpread.g, getterCalls);
console.log('getter is data  ', Object.getOwnPropertyDescriptor(getterSpread, 'g').get === undefined);

// A source that was extended with more keys after it was first built: the shape it ends on has
// to be the one actually copied, not a stale one from before the extension.
const extended = { a: 1, b: 2 };
extended.c = 3;
extended.d = 4;
console.log('extended        ', JSON.stringify({ ...extended }));

// A source that had a key deleted: deleting converts the object off the fast shaped
// representation, so this exercises the existing per-property path deliberately, the same way
// the getter case above does.
const deleted = { a: 1, b: 2, c: 3 };
delete deleted.b;
console.log('after delete    ', JSON.stringify({ ...deleted }));
console.log('after delete keys', JSON.stringify(Object.keys({ ...deleted })));

// `JSON.stringify` over the fast-path result, once more standing on its own rather than folded
// into an earlier line, so a future change to the string-building path is checked here too.
console.log('json            ', JSON.stringify({ ...base, f: 1, g: 2 }));

// The benchmark's own pattern and its checksum, computed the same way
// bench/non-io/object-spread.mjs does, so a native/interpreter divergence in the exact shape the
// benchmark exercises shows up here rather than only in a timing run.
{
  const benchBase = { a: 1, b: 'hello', c: true, d: [1, 2], e: 42 };
  let count = 0;
  for (let i = 0; i < 50000; i++) {
    const m = { ...benchBase, f: i, g: i * 2 };
    count += m.f + m.a;
  }
  console.log('bench checksum  ', count);
}
