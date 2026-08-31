// `in`, `delete`, `for...in` and the spread call. Each is a question about a property rather than
// a read of one: `in` asks whether the name exists without calling an accessor, and `delete` is
// about the state afterwards rather than about work done, which is why deleting a name that was
// never there succeeds.
const o: any = { a: 1, b: 2, c: 3 };
console.log("a" in o, "z" in o, "toString" in o);
console.log(delete o.a, "a" in o, delete o.zzz, o);

// Enumeration order is insertion order with integer keys first, and it is a snapshot.
const ordered: any = { b: 1, "2": 2, a: 3, "1": 4 };
const seen: string[] = [];
for (const key in ordered) { seen.push(key); }
console.log(seen, Object.keys(ordered), seen.join() === Object.keys(ordered).join());

// An array enumerates its indices, as strings.
const list = [7, 8, 9];
const indices: string[] = [];
for (const index in list) { indices.push(index); }
console.log(indices, 0 in list, 9 in list, "length" in list);

// `delete` on an *array element* leaves a hole, which this backend cannot represent -- see the
// note in NOTES.md and `conformance/differential/array-holes.js`, which covers it against node.

// A computed delete, and one through a nested object.
const nested: any = { inner: { x: 1, y: 2 } };
const which = "x";
console.log(delete nested.inner[which], nested.inner, "x" in nested.inner, "y" in nested.inner);

// A spread call: the arity comes from an array the program built, not from the bytecode.
function sum(...parts: number[]): number { return parts.reduce((total: number, n: number) => total + n, 0); }
console.log(sum(), sum(1, 2, 3), sum(...[4, 5]), sum(1, ...[2, 3], 4));
console.log(Math.max(...[1, 9, 3]), Math.min(...[5, 2]), [].concat(...[[1], [2]]));

// Rest after named parameters, and a spread of a generator.
function tagged(first: string, ...rest: number[]): string { return `${first}:${rest.join("-")}`; }
function* two(): Generator<number> { yield 1; yield 2; }
console.log(tagged("x", 1, 2, 3), tagged("y"), tagged("z", ...two()));

// `in` walks the prototype chain, which is what makes it different from `Object.hasOwn`.
class Base { method(): number { return 1; } }
const instance = new Base();
console.log("method" in instance, Object.hasOwn(instance, "method"), "nope" in instance);

// `GetV` boxes every non-null primitive before looking up a property. An unknown name is absent
// and therefore reads as undefined; it is not a native-backend refusal. This matters especially
// for recursive package code using `node.value?.member`, where `value` can legitimately be a
// number on one branch and an object on another.
console.log((97 as any).missing, (true as any).missing, (1n as any).missing);
const primitiveNode: any = { value: 97 };
console.log(primitiveNode.value?.stack, primitiveNode.value?.stack?.length);
