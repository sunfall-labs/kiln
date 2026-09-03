// `{ ...source }` over every kind of source that carries own properties.
//
// Object spread is not `Object.assign` with different syntax only by accident: it copies the same
// set -- own, enumerable, values not accessors -- from *any* source, and the two shapes it kept
// getting wrong are the two that carry their properties somewhere other than a plain object's
// store. A function's statics live on the function, and a proxy has no store at all and answers
// through traps. Both used to spread as `{}`, silently, which is the failure mode a spread can
// least afford: the result is a real object with real code reading real names off it, so the
// mistake surfaces somewhere else entirely. A bundled `postcss` builds its helper table with
// `{ ...postcss }` where `postcss` is a function carrying its plugins.
//
// Every line here is what node prints; the interpreter was checked against node directly rather
// than against itself, because this corpus uses the interpreter as its oracle and a wrong
// interpreter would happily be agreed with.

// A function is an object, and only what a program hung on it is enumerable. `name`, `length` and
// `prototype` are the function's own and are all non-enumerable, which is why the pile of function
// internals does not appear.
function tagged() {}
tagged.x = 1;
tagged.plugin = 'postcss';
console.log('function        ', JSON.stringify({ ...tagged }));
console.log('function keys   ', JSON.stringify(Object.keys({ ...tagged })));

// The same claim, stated rather than assumed: each of the three is an own property of the
// function, each is non-enumerable, and none of them survives the spread. Verifying it matters
// because "the spread is empty" and "the spread is right" would otherwise look alike.
for (const name of ['name', 'length']) {
  const descriptor = Object.getOwnPropertyDescriptor(tagged, name);
  console.log(
    'non-enumerable  ',
    name,
    descriptor !== undefined,
    descriptor.enumerable,
    name in { ...tagged },
  );
}
// `prototype` is checked only on the *result*. Both of the other ways to ask whether a function
// owns one -- a descriptor and `in` -- are native gaps of their own, and a probe that refuses or
// diverges on a neighbouring feature has stopped measuring what it was written to measure.
console.log('non-enumerable   prototype', 'prototype' in { ...tagged });

// A class constructor is a function with statics, and a bound function keeps its own store.
class Widget {
  static kind = 'widget';
  static make() {}
}
console.log('class statics   ', JSON.stringify(Object.keys({ ...Widget })));
const bound = tagged.bind(null);
bound.extra = 2;
console.log('bound           ', JSON.stringify({ ...bound }));

// A proxy is enumerated by `ownKeys`, filtered by `getOwnPropertyDescriptor`, and read by `get`.
// All three are observable, so a handler that counts is the check that none of them was skipped.
const seen = [];
const watched = new Proxy(
  { a: 1, b: 2 },
  {
    ownKeys(target) {
      seen.push('ownKeys');
      return Reflect.ownKeys(target);
    },
    getOwnPropertyDescriptor(target, key) {
      seen.push('descriptor:' + String(key));
      return Reflect.getOwnPropertyDescriptor(target, key);
    },
    get(target, key, receiver) {
      seen.push('get:' + String(key));
      return Reflect.get(target, key, receiver);
    },
  },
);
console.log('proxy           ', JSON.stringify({ ...watched }));
console.log('proxy traps     ', JSON.stringify(seen));

// A trapless proxy still forwards, and a proxy whose target is a function spreads the function's
// statics -- the source's kind is the target's, not the handler's.
console.log('proxy bare      ', JSON.stringify({ ...new Proxy({ a: 1 }, {}) }));
console.log('proxy over fn   ', JSON.stringify({ ...new Proxy(tagged, {}) }));

// A proxy may hide a key by reporting it non-enumerable, and it may answer a read with something
// its descriptor never mentioned: a spread copies what the *read* produced.
const inventive = new Proxy(
  { shown: 1, hidden: 2 },
  {
    getOwnPropertyDescriptor(target, key) {
      const descriptor = Reflect.getOwnPropertyDescriptor(target, key);
      return key === 'hidden' ? { ...descriptor, enumerable: false } : descriptor;
    },
    get() {
      return 'from get';
    },
  },
);
console.log('proxy filtered  ', JSON.stringify({ ...inventive }));

// A string's indices are own enumerable properties, so they are what a spread of one produces.
// `length` is not among them, for the same reason a function's is not.
console.log('string          ', JSON.stringify({ ...'ab' }));
// A boxed String is deliberately absent: `new String` is a native gap of its own, and a probe
// that refuses on a neighbouring feature stops measuring the one it was written for.

// Nullish contributes nothing rather than throwing, which is the one place object spread is more
// forgiving than array spread. A number, a boolean and a symbol have nothing of their own.
console.log('null            ', JSON.stringify({ ...null }));
console.log('undefined       ', JSON.stringify({ ...undefined }));
console.log('number          ', JSON.stringify({ ...5 }));
console.log('boolean         ', JSON.stringify({ ...true }));
console.log('symbol          ', JSON.stringify({ ...Symbol('s') }));

// An array spreads as its indices, and a hole is skipped -- a different operation from array
// spread, which reads the iterator and turns a hole into undefined. A name hung on the array is
// an own enumerable property too and belongs in the result; `length` does not.
const counted = [1, 2];
counted.total = 3;
console.log('array           ', JSON.stringify({ ...[1, 2] }));
console.log('array holes     ', JSON.stringify({ ...[1, , 3] }));
console.log('array named     ', JSON.stringify({ ...counted }));
console.log('typed array     ', JSON.stringify({ ...new Uint8Array([7, 8]) }));

// A getter is *called*, once, and what it produced is stored as a plain data property. The target
// gets a value, not the accessor: writing to the copy must not reach the source.
let reads = 0;
const withGetter = {
  plain: 1,
  get computed() {
    reads += 1;
    return 'called ' + reads;
  },
};
const copied = { ...withGetter };
console.log('getter          ', JSON.stringify(copied), reads);
console.log(
  'getter is data  ',
  Object.getOwnPropertyDescriptor(copied, 'computed').get === undefined,
  Object.getOwnPropertyDescriptor(copied, 'computed').writable,
);
copied.computed = 'overwritten';
console.log('getter detached ', copied.computed, reads);

// A setter with no getter still produces a key. `CopyDataProperties` creates a property for every
// enumerable own key it was handed, whatever the read answered, and a read with no getter answers
// `undefined` -- so the key is present and its value is undefined, which `JSON.stringify` then
// drops. Skipping it left the key genuinely absent, which is a different object.
const setterOnly = {
  set gone(_value) {
    throw new Error('a spread must not call a setter on the source');
  },
};
console.log('setter only     ', JSON.stringify({ ...setterOnly }), 'gone' in { ...setterOnly });

// Symbol-keyed own enumerable properties are copied too, which is where spread and `Object.keys`
// part company: `Object.keys` never reports a symbol and a spread always carries one across.
//
// Last in the file on purpose. The native backend refuses here rather than answering, because its
// property store marks every reserved `@@` key non-enumerable in one place so that no walk over
// own string keys has to know about the prefix -- which leaves it unable to say whether a program
// made a symbol key enumerable or hid it. A refusal stops the probe, so everything above it has
// already been compared by the time this is reached.
const marker = Symbol('marker');
const fromSymbol = { [marker]: 'kept', plain: 1 };
const symbolCopy = { ...fromSymbol };
console.log('symbol key      ', symbolCopy[marker], JSON.stringify(Object.keys(symbolCopy)));

// A symbol hung on a function is a static like any other, and travels with the function's store.
function carrier() {}
carrier[marker] = 'static';
console.log('symbol on fn    ', { ...carrier }[marker]);

// A symbol the program hid with a descriptor is left behind exactly as a hidden string key is.
const hiddenSymbol = {};
Object.defineProperty(hiddenSymbol, marker, { value: 'no', enumerable: false });
console.log('symbol hidden   ', { ...hiddenSymbol }[marker]);
