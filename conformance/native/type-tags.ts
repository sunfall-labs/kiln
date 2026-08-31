// `Object.prototype.toString` reports what a value *is* rather than converting it, which is why it
// survived every type test built since: it is the one operation that tells an array from an
// arguments object without asking either of them a question. A library still reaches for it, so
// both engines have to spell the same tag, and `Symbol.toStringTag` is the supported way a program
// changes what one says about its own objects.

const tag = Object.prototype.toString;
console.log(tag.call({}), tag.call([]), tag.call(null), tag.call(undefined));
console.log(tag.call(1), tag.call("s"), tag.call(true), tag.call(NaN));
console.log(tag.call(function named() {}), tag.call(() => {}), tag.call(class {}));
console.log(tag.call(new Date(0)), tag.call(/x/g), tag.call(new Map()), tag.call(new Set()));
console.log(tag.call(Math), tag.call(JSON), tag.call(Promise.resolve()));

// The tag is not the constructor's name: a class instance is a plain object to this operation,
// which is exactly why `Symbol.toStringTag` had to be added.
class Plain { value = 1; }
console.log(tag.call(new Plain()), new Plain().constructor.name);

// A `Symbol.toStringTag` on the object itself, and one inherited from a class as an accessor --
// the usual spelling, because a getter on the prototype costs nothing per instance.
const declared = { [Symbol.toStringTag]: "Declared" };
console.log(tag.call(declared), String(declared), `${declared}`);
class Named { get [Symbol.toStringTag](): string { return "Named"; } }
console.log(tag.call(new Named()), String(new Named()));

// It changes the tag and nothing else: the property is not enumerated as a string key, and the
// object is still an ordinary object to everything that reads it.
console.log(Object.keys(declared), JSON.stringify(declared), typeof declared);

// A tag that is not a string is ignored, and the built-in answer comes back.
const wrong: any = { [Symbol.toStringTag]: 42 };
console.log(tag.call(wrong));

// What is deliberately *not* asked here. `Object.prototype.toString.call(new Error("e"))` is
// `[object Error]` in node and `[object Object]` in both kiln engines, and
// `.call(Symbol("s"))` is `[object Symbol]` in node and the native backend but `[object Object]`
// in the interpreter -- as is `.call(10n)`, which is `[object BigInt]` everywhere else. The first
// is the pair being wrong together, which is a language gap rather than a backend one; the others
// are the interpreter being wrong on its own, and this corpus takes the interpreter as its oracle,
// so recording any of them would record a wrong answer. See NOTES.md.
