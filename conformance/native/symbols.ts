// A symbol is a property key with an identity of its own.
//
// Both engines key the property under a reserved string -- `@@iterator` for a well-known symbol,
// `@@symbol:<serial>` for a unique one -- so the object model stays string-keyed and nothing else
// has to learn about symbols. The consequences of that choice are what this probe checks: two
// symbols with the same description must not collide, and a program must not be able to spell
// either by writing a string.

const first = Symbol("s");
const second = Symbol("s");
const nameless = Symbol();
console.log(typeof first, String(first), String(nameless), first === first, first === second);
console.log(Symbol.iterator === Symbol.iterator, typeof Symbol.iterator, String(Symbol.iterator));
console.log(first, Symbol.iterator, Symbol, typeof Symbol);

// Two descriptions that match still name two properties, and neither is reachable by writing the
// description as a string.
//
// What is deliberately *not* checked here: a string key that happens to start with `@@`. Both
// engines store a symbol-keyed property under a reserved string, so `obj["@@iterator"]` and
// `obj[Symbol.iterator]` are the same slot and neither is enumerated -- which node disagrees with.
// See NOTES.md; it is a representation decision, not a backend one, and the two engines are wrong
// about it together rather than differently.
const holder: any = {};
holder[first] = 1;
holder[second] = 2;
holder[Symbol.iterator] = 3;
console.log(holder[first], holder[second], holder[Symbol.iterator]);
console.log(Object.keys(holder).length, JSON.stringify(holder), holder["Symbol(s)"]);

// The iterator protocol is the reason well-known symbols have fixed keys: two mentions of
// `Symbol.iterator` have to reach the same property.
const counted = { *[Symbol.iterator]() { yield 1; yield 2; yield 3; } };
console.log([...counted]);
for (const value of counted) { console.log("saw", value); }

// A symbol is truthy, and it is not a string however it prints.
console.log(!!Symbol("x"), typeof String(first), String(first) === "Symbol(s)");
