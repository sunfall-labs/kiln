// Plain data objects and arrays. Everything here is own-property access: a prototype, an
// accessor, a method call and `delete` are all still refused by the native backend, by name.
const point = { x: 1, y: 2 };
console.log(point, point.x, point.y, point.z);
point.x = 10;
point.z = 3;
console.log(point, point.x + point.y + point.z);

const list = [1, 2, 3];
console.log(list, list[0], list[2], list[3], list.length);
list[1] = 20;
// Writing past the end grows the array and leaves undefined behind.
list[4] = 5;
console.log(list, list.length);

// A console shows a value rather than converting it, so the two engines have to agree on the
// format character for character: quoted keys, quoted nested strings, and a placeholder past
// two levels of nesting.
console.log({ a: { b: [1, { c: 2 }] }, s: "it's", t: "a\nb", u: "" });
console.log({}, [], { "a-b": 1, _y: 2, $z: 3, "0": 4 }, ["x", true, null, undefined]);

// A computed key is a string, whatever it was written as -- and an array index is a key that
// round-trips through its canonical spelling, which is what keeps `a["01"]` a named property.
const key = "y";
console.log(point[key], list["1"], point["x"], list[1.0]);

// The conversions an object goes through, which are not the console's.
console.log(String(point), `${list}`, point + "", [] + "", [1, 2] + "", !point, !list);
console.log(typeof point, typeof list, point === point, point === { x: 10, y: 2, z: 3 });

// Objects and functions together: a function stored in an object still prints as one.
function named(): number { return 1; }
console.log({ f: named, n: [named] });

// Objects built from computed values, and one holding another.
function makePair(a: number, b: number) {
  return { first: a, second: b, sum: a + b };
}
const pair = makePair(3, 4);
console.log(pair, pair.sum, { wrapped: pair }.wrapped.first);
