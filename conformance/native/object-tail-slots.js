// A known-size object literal uses in-block slots for its ninth through twelfth properties.
// Growing it must copy those slots before reallocating, and dictionary conversion must not
// free the tail of the GC block as though it were a separate malloc allocation.
const value = {
  a: 0, b: 1, c: 2, d: 3, e: 4, f: 5,
  g: 6, h: 7, i: 8, j: 9, k: 10, l: 11,
};
console.log(Object.keys(value).length, value.a, value.l);
value.m = 12;
console.log(Object.keys(value).length, value.l, value.m);
delete value.b;
console.log("b" in value, value.l, value.m);
const copy = { ...value, n: 13 };
console.log(Object.keys(copy).length, copy.l, copy.m, copy.n);
Object.defineProperty(value, "o", { value: 14, writable: true, enumerable: true });
console.log(Object.keys(value).length, value.o, value.l);
