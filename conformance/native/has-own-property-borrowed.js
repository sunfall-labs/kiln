// `Object.prototype.hasOwnProperty.call(x, k)` against every receiver kind, and the same question
// asked as `Object.hasOwn(x, k)`.
//
// WHY THE `.call` SPELLING RATHER THAN `x.hasOwnProperty(k)`. Every bundler writes it this way --
// the method is borrowed off `Object.prototype` once and applied to whatever turns up -- so the
// receiver reaching the implementation is arbitrary rather than an ordinary object. Three bundled
// libraries stopped natively on exactly that: `lodash` asking a `Map` for `Symbol.toStringTag`,
// `immer` asking `Array.prototype` for `constructor`, `mobx` asking whatever `hasProp` was handed.
// All three refused, at three different lines, on the one receiver test the implementation had.
//
// WHY THE PAIR IS CHECKED TOGETHER. `hasOwnProperty` and `Object.hasOwn` are one question with two
// spellings, and they had been written twice and drifted: only one of them answered for a
// function, only one followed a proxy's trap, and only one converted a symbol key to the slot a
// symbol actually keys. Asking both of everything is what keeps them from drifting again.
//
// Kinds deliberately absent: a string, a regular expression, an `ArrayBuffer` and a `DataView`.
// Each of those has own properties the two engines do not yet answer alike about, so the backend
// refuses rather than picking one of the two answers -- and a refusal here would stop the probe.

const has = Object.prototype.hasOwnProperty;
const both = (label, subject, key) => {
  console.log(label, has.call(subject, key), Object.hasOwn(subject, key));
};

// An ordinary object, which is the case that always worked, and the two keys beside it: a symbol
// keys a property under a slot of its own, so converting it to the text it *prints* as asked
// after a property named "Symbol(Symbol.toStringTag)" and answered false for one that was there.
const plain = Symbol("plain");
const tagged = { a: 1, [plain]: 2, [Symbol.toStringTag]: "W" };
both("object own", tagged, "a");
both("object absent", tagged, "b");
both("object symbol", tagged, plain);
both("object well-known", tagged, Symbol.toStringTag);
both("object inherited", Object.create({ up: 1 }), "up");

// An array: live indices, `length`, and a named property beside them; a hole is not own.
const sparse = [1, , 3];
sparse.extra = 4;
both("array index", sparse, "0");
both("array hole", sparse, "1");
both("array length", sparse, "length");
both("array named", sparse, "extra");

// A function owns `name`, `length` and -- unless it is an arrow -- `prototype`, plus whatever was
// assigned beside them. `prototype` is the one the descriptor path cannot answer, because whether
// it is writable depends on how the function was written; presence is the weaker question and is
// knowable either way.
function declared(first, second) {
  return first + second;
}
declared.decorated = true;
both("function name", declared, "name");
both("function length", declared, "length");
both("function prototype", declared, "prototype");
both("function assigned", declared, "decorated");
both("function absent", declared, "missing");
both("arrow prototype", () => 0, "prototype");
both("bound name", declared.bind(null), "name");

// A collection, a promise, a date and a generator carry every member on the prototype and can be
// given none of their own, so false is the whole answer rather than an admission of ignorance.
//
// A promise and a date are asked about a name nothing carries rather than about `then` and
// `getTime`. That is not squeamishness: the *interpreter* answers `hasOwnProperty` for those two
// kinds by walking the prototype chain, so it says true for an inherited method where its own
// `Object.hasOwn` says false and node says false. Until that is settled there is no answer this
// probe could assert.
function* counting() {
  yield 1;
}
both("map size", new Map([["k", 1]]), "size");
both("set size", new Set([1]), "size");
both("weakmap absent", new WeakMap(), "anything");
both("promise absent", Promise.resolve(1), "anything");
both("date absent", new Date(0), "anything");
both("generator absent", counting(), "next");
both("map toStringTag", new Map(), Symbol.toStringTag);

// Primitives go through `ToObject`, and the wrapper owns nothing: `description` and `toFixed` are
// both the prototype's.
both("number absent", 1, "toFixed");
both("boolean absent", true, "valueOf");
both("bigint absent", 1n, "toString");
both("symbol absent", plain, "description");

// A typed array owns its indices and not its `length`, which is the accessor on the shared
// prototype -- the one pair that separates a view from an array.
const view = new Uint8Array([7, 8]);
both("typed index", view, "1");
both("typed past end", view, "2");
both("typed length", view, "length");

// A built-in answers for the members it carries. A name the table does not know refuses rather
// than answering false, because the table is still growing and a short list read as a complete
// one is the wrong answer this backend is not allowed to give -- so only reachable names are
// asked here.
both("Object.prototype method", Object.prototype, "hasOwnProperty");
both("Array.prototype method", Array.prototype, "slice");
both("Array.prototype constructor", Array.prototype, "constructor");
both("Object static", Object, "keys");
both("Math member", Math, "floor");
both("JSON member", JSON, "parse");

// A proxy answers through its `getOwnPropertyDescriptor` trap and never through the target's
// store, which is the difference a copy-on-write layer depends on.
const target = { visible: 1 };
const trapped = new Proxy(target, {
  getOwnPropertyDescriptor(owner, key) {
    if (key === "invented") {
      return { value: 9, enumerable: true, configurable: true };
    }
    return Object.getOwnPropertyDescriptor(owner, key);
  },
});
both("proxy real", trapped, "visible");
both("proxy invented", trapped, "invented");
both("proxy absent", trapped, "neither");

// `ToObject(this)` comes first and there is nothing to convert these two to, so both spellings
// throw rather than answering.
try {
  has.call(null, "a");
  console.log("null did not throw");
} catch (error) {
  console.log("null threw", error instanceof TypeError);
}
try {
  has.call(undefined, "a");
  console.log("undefined did not throw");
} catch (error) {
  console.log("undefined threw", error instanceof TypeError);
}

// `Function.prototype.apply` takes an array-*like*, not an array. Testing for an array alone let
// `f.apply(this, arguments)` -- how every wrapper forwards a call -- run the function with no
// arguments at all and answer with whatever the missing parameters defaulted to. That is a silent
// wrong answer rather than a refusal, so it belongs beside the one above.
function shape(first, second, third) {
  return `${first}/${second}/${third}`;
}
function forwarded() {
  return shape.apply(null, arguments);
}
console.log("apply array", shape.apply(null, [1, 2, 3]));
console.log("apply arguments", forwarded(1, 2, 3));
console.log("apply array-like", shape.apply(null, { length: 2, 0: "x", 1: "y" }));
console.log("apply short length", shape.apply(null, { length: 1, 0: "x", 1: "y" }));
console.log("apply nothing", shape.apply(null));
console.log("apply null", shape.apply(null, null));
console.log("apply typed", shape.apply(null, new Uint8Array([4, 5])));
