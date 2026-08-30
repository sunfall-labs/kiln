// The array methods the native backend did not have, and the reflective statics that go with
// them. Every one of these ran in the interpreter and stopped the backend at runtime, which is a
// refusal in the wrong place: the emitter accepted the program and the runtime could not finish
// it.

const numbers = [5, 1, 4, 2, 3];
console.log(numbers.at(0), numbers.at(-1), numbers.at(9), [].at(0));
console.log(numbers.find((x) => x > 3), numbers.findIndex((x) => x > 3));
console.log(numbers.find((x) => x > 9), numbers.findIndex((x) => x > 9));
console.log(numbers.some((x) => x > 4), numbers.every((x) => x > 0));
console.log([].some(() => true), [].every(() => false), numbers.lastIndexOf(3), [1,2,1].lastIndexOf(1));

// Without a comparator the elements are compared *as text*, which is the language's oldest
// surprise; the sort is stable, and stability is program-visible rather than an implementation
// detail.
console.log([...numbers].sort().join(","), [...numbers].sort((l, r) => l - r).join(","));
console.log([10, 9, 1].sort().join(","), [3, 1, 2].sort((l, r) => r - l).join(","));
const pairs = [{ k: 1, v: "a" }, { k: 1, v: "b" }, { k: 0, v: "c" }];
console.log(pairs.sort((l, r) => l.k - r.k).map((x) => x.v).join(""));
const inPlace = [3, 1, 2];
console.log(inPlace.sort() === inPlace, inPlace.join(","));

// A hole sorts to the end behind undefined, and `find` sees one where `some` skips it.
const holes = [1, , 3];
console.log(holes.find((x) => x === undefined), holes.some((x) => x === undefined));
console.log(holes.every((x) => x !== undefined), JSON.stringify(holes.flat()));
console.log(JSON.stringify([3, 1, , 2].sort()), [3, 1, , 2].sort().join(","));

console.log([1, [2, [3, [4]]]].flat().length, JSON.stringify([1, [2, [3, [4]]]].flat(2)));
console.log(JSON.stringify([1, [2]].flat(0)), JSON.stringify([1, [2, [3]]].flat(Infinity)));
console.log([...[1, 2].entries()], [...[1, 2].keys()], JSON.stringify([...["a", "b"].entries()]));

const mutable = [1, 2, 3];
console.log(mutable.reverse().join(","), mutable.shift(), mutable.join(","));
console.log(mutable.unshift(9, 8), mutable.join(","), [].shift());
console.log([1, 2, 3, 4].fill(0, 1, 3).join(","), [1, 2, 3].fill(7).join(","));

// The reflective statics. `Object.getPrototypeOf({})` answers `Object.prototype`, and an object
// asked for with no prototype answers null -- undefined and null are different answers here, one
// meaning nothing was recorded and the other meaning nothing was wanted.
console.log(Object.getPrototypeOf({}) === Object.prototype, Object.getPrototypeOf([]) === Array.prototype);
console.log(Object.getPrototypeOf(Object.create(null)), Object.getPrototypeOf(Object.create({ a: 1 })).a);
const base = { greet() { return "hi"; } };
const child = Object.setPrototypeOf({}, base);
console.log(child.greet(), Object.getPrototypeOf(child) === base, base.isPrototypeOf(child));
console.log(base.isPrototypeOf({}), base.isPrototypeOf(Object.create(Object.create(base))));

// `getOwnPropertyNames` differs from `keys` only for non-enumerable properties, which is how the
// interop shims copy a module whose exports are a function.
function tagged(): void {}
(tagged as any).chunk = 1;
(tagged as any).map = 2;
const names = Object.getOwnPropertyNames(tagged);
console.log(names.includes("chunk"), names.includes("map"), Object.getOwnPropertyNames({ a: 1, b: 2 }).join(","));
console.log(JSON.stringify(Object.fromEntries([["a", 1], ["b", 2]])));
console.log(JSON.stringify(Object.fromEntries(Object.entries({ x: 1 }))));
console.log(Array.from("abc").join(","), Array.from([1, 2], (x) => x * 2).join(","));
console.log(Array.from(new Set([1, 1, 2])).join(","), Array.from(new Map([[1, "a"]])).length);

// The seven neither engine had. These are not exotic: `splice` appears in three of the thirdparty
// bundles, `flatMap` in two, and `reduceRight` and `findLast` in one each. The probes that pass do
// not reach those lines, which is the argument for reading a corpus as a lower bound.
console.log([1, 2, 3].flatMap((x) => [x, x * 2]).join(","), [1, 2].flatMap((x) => x).join(","));
console.log([[1], [2]].flatMap((x) => x).join(","), [1, 2].flatMap((x) => [[x]]).length);
console.log([1, 2, 3, 4].findLast((x) => x % 2 === 1), [1, 2, 3, 4].findLastIndex((x) => x % 2 === 1));
console.log([1, 2].findLast((x) => x > 9), [1, 2].findLastIndex((x) => x > 9));
console.log([1, 2, 3].reduceRight((l, r) => l + r), [1, 2, 3].reduceRight((l, r) => `${l}${r}`));
console.log(["a", "b", "c"].reduceRight((l, r) => l + r, "-"), [].reduceRight((l: number, r: number) => l + r, 7));
const order: number[] = [];
[1, 2, 3].reduceRight((carried, _value, index) => { order.push(index); return carried; }, 0);
console.log(order.join(","));

// `splice` returns what it removed, which is what makes it usable as a queue read. A missing count
// removes everything from the start; an explicit `undefined` removes nothing.
const spliced = [1, 2, 3, 4, 5];
console.log(spliced.splice(1, 2).join(","), spliced.join(","));
const inserted = [1, 2, 3];
console.log(inserted.splice(1, 0, 9, 8).length, inserted.join(","));
const tail = [1, 2, 3];
console.log(tail.splice(1).join(","), tail.join(","));
const fromEnd = [1, 2, 3];
console.log(fromEnd.splice(-2, 1).join(","), fromEnd.join(","));
const explicit = [1, 2, 3];
console.log(explicit.splice(1, undefined).length, explicit.join(","), [].splice(0, 1).length);

// The source and the destination of `copyWithin` overlap by design; that overlap is the whole
// reason the method exists.
console.log([1, 2, 3, 4, 5].copyWithin(0, 3).join(","), [1, 2, 3, 4, 5].copyWithin(1, 3, 4).join(","));
console.log([1, 2, 3].copyWithin(-2, 0).join(","), [1, 2, 3].copyWithin(0, -2).join(","));
console.log([1, 2].toString(), String([1, [2, 3]]), [].toString() === "", [1, 2].toString() === [1, 2].join(","));
