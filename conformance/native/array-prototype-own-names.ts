// `Object.getOwnPropertyNames(Array.prototype)` has to name the same members on both engines: a
// listing is how a polyfill decides whether a realm already has a member, and a name missing
// from it while a read answers is the divergence that decides wrongly. The whole sorted list is
// printed rather than a count, so a future gap names itself in the diff instead of arriving as
// "39 != 40".
//
// `values` was the name this probe was written for. It is the same function `@@iterator` hands
// out, an array instance had said so since the iteration protocol was written down, and the
// prototype object had not: it is numbered in the generic method group rather than the
// array-lookup one, so the name never led back to its own code and the walk that lists a
// prototype's members skipped it. `Array.prototype.values` refused where `[].values` is a
// function.
const names = Object.getOwnPropertyNames(Array.prototype).sort();
console.log(names.length);
console.log(names.join(","));

// The three iterator members read as callables, and `values` is `@@iterator` rather than a
// second function that happens to walk the same way.
console.log(typeof (Array.prototype as any).values, typeof (Array.prototype as any).keys, typeof (Array.prototype as any).entries);
console.log((Array.prototype as any).values === (Array.prototype as any)[Symbol.iterator]);
console.log(typeof [].values, [].values === (Array.prototype as any).values);

// A name in the listing is a name the own-property questions answer, which is the half a listing
// that names something unreadable would get wrong in the other direction. `in` is deliberately
// not asked here: on this backend `"map" in Array.prototype` is false for every member of it,
// where `hasOwnProperty` is true, and on the interpreter it is true for every name including one
// no engine has. Both are older and wider than this probe and are filed with it.
console.log(Object.prototype.hasOwnProperty.call(Array.prototype, "values"));
console.log(Object.prototype.hasOwnProperty.call(Array.prototype, "keys"));
console.log(Object.prototype.hasOwnProperty.call(Array.prototype, "entries"));

// Borrowed onto a receiver of its own, the way every other generic member of this prototype is.
console.log(JSON.stringify(Array.from((Array.prototype as any).values.call([7, 8]))));
console.log(JSON.stringify(Array.from((Array.prototype as any).keys.call(["a", "b"]))));
