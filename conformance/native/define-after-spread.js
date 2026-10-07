// `DefineNamedProperty` -- `{ ...base, f: i, g: i * 2 }`'s per-key definition after a spread --
// gets a shaped fast path (kiln_c99_define_named_fast in spread.c): when the accumulator is still
// a plain shaped object (no properties list, not frozen, not non-extensible, no accessors) and the
// key is a string, it interns the key once and either overwrites the shape's existing slot or
// takes one transition, storing the NaN-boxed value directly with no trip through the boundary
// `kiln_value`. Every value type below has to come out unchanged whether it went through that
// path or, for the cases the fast path deliberately misses, the generic `kiln_c99_put` -- a
// function value (which still needs `kiln_c99_set_function_name`), `__proto__`'s literal spelling
// (which relinks the prototype instead of defining a property), a duplicate key, a key past the
// point the shape tree keeps growing fast slots, and a computed or symbol key never reach the
// fast path's string-key check at all.
//
// Checked against node directly, the way object-spread-shape.js is: this corpus otherwise uses
// the interpreter as its oracle, and a wrong interpreter would happily agree with a wrong native
// backend.

const base = { a: 1, b: 2 };

// Named keys after a spread, one property at a time, across the value types a slot has to carry
// unconverted: string, number, and object.
const obj = { x: 1 };
const withValues = { ...base, str: 'hi', num: 42, obj };
console.log('value types     ', JSON.stringify(withValues));
console.log('object identity ', withValues.obj === obj);

// An anonymous function value, written directly as a property so `SetFunctionName` has something
// to do: this key deliberately misses the fast path (a function value still needs
// `kiln_c99_set_function_name`), and the property's key is what names it.
const withFn = { ...base, fn: function () {} };
console.log('function name   ', typeof withFn.fn, withFn.fn.name);

// A computed key after a spread: never reaches the fast path (it only fires for the module's own
// static-string key constants), same generic path as before.
const keyName = 'computed' + 'Key';
const withComputed = { ...base, [keyName]: 'value' };
console.log('computed key    ', JSON.stringify(withComputed));

// A symbol used as a computed key after a spread: also never a string key, so this exercises the
// generic path's own-slot handling exactly as it did before the fast path existed.
const sym = Symbol('after-spread');
const withSymbolKey = { ...base, [sym]: 'symbolic', c: 3 };
console.log('symbol as key   ', withSymbolKey[sym], JSON.stringify(withSymbolKey));
if (Object.keys(withSymbolKey).length !== 3 ||
    !Object.getOwnPropertySymbols(withSymbolKey).includes(sym)) {
  throw new Error('computed symbol key after spread became an enumerable string key');
}

const symbolA = Symbol.for('record-symbol-a');
const symbolB = Symbol.for('record-symbol-b');
const symbolSource = { [symbolA]: 1, [symbolB]: 2 };
const symbolUpdate = { ...symbolSource, [symbolA]: 'updated' };
if (Object.keys(symbolUpdate).length !== 0 ||
    Object.getOwnPropertySymbols(symbolUpdate).length !== 2 ||
    symbolUpdate[symbolA] !== 'updated' || symbolUpdate[symbolB] !== 2) {
  throw new Error('computed symbol overwrite after symbol spread lost property identity');
}

// A duplicate key: the same name spread from the source, then written again by the literal --
// later must win, and neither the fast path's slot-overwrite branch nor a shape transition may
// duplicate or reorder the key.
const withDuplicate = { ...base, a: 100, a: 200 };
console.log('duplicate later ', JSON.stringify(withDuplicate));
console.log('duplicate keys  ', JSON.stringify(Object.keys(withDuplicate)));

// `__proto__` spelled as a literal key after a spread: this is the one static spelling the
// generic path relinks the prototype for instead of defining an own property, and the fast path
// explicitly defers to it rather than duplicate the check.
const customProto = { greeting() { return 'from proto'; } };
const withProto = { ...base, __proto__: customProto };
console.log('proto relinked  ', Object.getPrototypeOf(withProto) === customProto);
console.log('proto own keys  ', JSON.stringify(Object.keys(withProto)));
console.log('proto method    ', withProto.greeting());

// A quoted `"__proto__"` literal key is the same spelling as the bareword one -- both are a
// non-computed `LiteralPropertyName` -- so this must relink exactly as the bareword form above,
// not become an own property named `__proto__`.
const withQuotedProto = { ...base, 'a': 1, '__proto__': customProto };
console.log('quoted proto    ', Object.getPrototypeOf(withQuotedProto) === customProto);

// A getter defined after a spread: an accessor can never live in a shaped object's slots, so this
// must take the generic path (kiln_c99_define_accessor, not DefineNamedProperty at all) and the
// fast path must never be reached for it.
let getterCalls = 0;
const withGetter = {
  ...base,
  get computed() {
    getterCalls += 1;
    return 'computed-value';
  },
};
console.log('getter value    ', withGetter.computed, getterCalls);
console.log('getter is data  ', Object.getOwnPropertyDescriptor(withGetter, 'computed').get === undefined);

// A literal that grows past the object's eight inline slots: `base` contributes 2, so eight more
// named keys after it exercises both an overwrite of an existing slot in the middle (`a` again)
// and enough transitions to force `kiln_c99_grow_slots`'s heap allocation, not just its inline
// array.
const wide = {
  ...base,
  c: 3,
  d: 4,
  e: 5,
  f: 6,
  g: 7,
  h: 8,
  i: 9,
  j: 10,
  a: 11,
};
console.log('wide literal    ', JSON.stringify(wide));
console.log('wide keys       ', JSON.stringify(Object.keys(wide)));

// The benchmark's own pattern once more, so a native/interpreter divergence in exactly what
// bench/non-io/object-spread.mjs exercises shows up here too.
{
  const benchBase = { a: 1, b: 'hello', c: true, d: [1, 2], e: 42 };
  let count = 0;
  for (let i = 0; i < 1000; i++) {
    const m = { ...benchBase, f: i, g: i * 2 };
    count += m.f + m.a;
  }
  console.log('bench checksum  ', count);
}
