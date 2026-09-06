// `Array.prototype.join`. A nullish element and a hole contribute nothing; everything else goes
// through `String()`, which is where a number, a boolean, a nested array and a plain object each
// get their text. Covered here without a separator (the default is a comma), with a non-string
// separator (itself coerced to text), and at the two sizes an inline-capacity change to arrays
// could disturb: no elements and one.
console.log([1, 2, 3].join());
console.log([1, true, false, null, undefined].join("-"));
const withHole = [1, , 3];
console.log(withHole.join(","));
console.log([[1, 2], [3, 4]].join(";"));
console.log([{ a: 1 }, { b: 2 }].join(","));
console.log([].join(","));
console.log([1].join(","));
console.log([1, 2].join(0));
console.log([1, 2].join(null));
console.log(["a", "b", "c", "d", "e", "f", "g", "h", "i"].join("-"));
