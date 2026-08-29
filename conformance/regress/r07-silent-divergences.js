// Four answers that were wrong without any diagnostic. Each was found by running a small program
// under both kiln and node and diffing, which is the only way this class of bug surfaces: a
// refusal is visible, a wrong answer is not.

// 1. A `let` in a for head is re-bound every iteration, so closures made in the body capture
//    that iteration's value. `var` is function-scoped and must still not be.
const fromLet = [];
for (let i = 0; i < 3; i++) fromLet.push(() => i);
const fromVar = [];
for (var j = 0; j < 3; j++) fromVar.push(() => j);
console.log(fromLet.map((f) => f()).join(), fromVar.map((f) => f()).join());

// 2. Own keys do not enumerate in insertion order: array indices come first, ascending, and
//    everything else follows in insertion order. `"01"` and `"-1"` are not indices.
const keyed = { b: 1, a: 2, 1: 3, 0: 4, '01': 5, 10: 6, 2: 7, '-1': 8 };
console.log(Object.keys(keyed).join(','));
console.log(JSON.stringify(keyed));
console.log(JSON.stringify({ ...keyed }));

// 3. `slice` counts a negative bound from the end. It was written for one internal caller that
//    only ever passed a non-negative start, and silently clamped everything else to zero.
const list = [1, 2, 3, 4, 5];
console.log(list.slice(-2).join(), list.slice(1, 3).join(), list.slice(-3, -1).join());
console.log(list.slice(3, 1).length, list.slice(10).length, list.slice(0, -1).join());

// 4. `JSON.parse` returned undefined for everything.
console.log(JSON.parse('{"a":[1,2],"b":{"c":null}}').a[1]);
console.log(JSON.parse('true'), JSON.parse('"s"'), JSON.parse('  -1.5e2  '), JSON.parse('[]').length);
console.log(JSON.stringify(JSON.parse('{"n":"a\\"b\\\\c\\u00e9\\n"}')));
const original = { a: 1, b: [true, null, 'x'], c: { d: 2.5 } };
console.log(JSON.stringify(JSON.parse(JSON.stringify(original))) === JSON.stringify(original));
