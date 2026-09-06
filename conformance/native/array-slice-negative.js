// `Array.prototype.slice` with negative indices, which count from the end, and the sizes on
// either side of the array's own eight-element inline storage -- a small array made through
// `kiln_c99_new_array_with_capacity` now hands out that inline block instead of a malloc, so its
// `slice` had better still behave at exactly eight, at one more, and at one fewer.
//
// Printed through `.join(",")` rather than handed straight to `console.log`: Node wraps a
// `console.log` of an array past six elements onto several lines with its own column width, which
// has nothing to do with `slice` and would make every one of these a false divergence.
const a = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
console.log(a.slice(-3).join(","));
console.log(a.slice(-100).join(","));
console.log(a.slice(2, -2).join(","));
console.log(a.slice(-2, 2).join(","));
console.log(a.slice(-3, -1).join(","));
console.log(a.slice(0, 8).join(","));
console.log(a.slice(0, 9).join(","));
console.log(a.slice(0, 7).join(","));
console.log(a.slice(0, 8).length, a.slice(0, 7).length, a.slice(0, 9).length);
console.log([].slice().join(","), [].slice().length);
console.log([1].slice(-1).join(","));
