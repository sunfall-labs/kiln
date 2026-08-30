// Array methods, including the ones that call back into the program -- which is the point:
// `map` running a closure the generated unit made is the whole call machinery exercised from
// inside the runtime rather than from emitted code.
const a = [1, 2, 3];
console.log(a.push(4), a, a.pop(), a, a.push(5, 6), a);
console.log(a.join("-"), a.join(), a.join(""), [].join("-"));
console.log(a.indexOf(2), a.indexOf(99), a.includes(3), a.includes(99));
console.log(a.slice(), a.slice(1), a.slice(1, 3), a.slice(-2), a.slice(2, 1), a.slice(-99));
console.log(a.concat([9], 10), a.concat(), [].concat([1], [2]));

console.log(a.map((x: number) => x * 2));
console.log(a.map((x: number, i: number) => `${i}:${x}`));
console.log(a.filter((x: number) => x % 2 === 1), a.filter(() => false));
console.log(a.reduce((sum: number, x: number) => sum + x, 0), a.reduce((s: number, x: number) => s + x));
console.log([].reduce((s: number, x: number) => s + x, 7));

let seen = "";
a.forEach((x: number, i: number) => { seen = `${seen}${i}=${x},`; });
console.log(seen);

// A closure over a captured binding, called from inside the runtime's own loop.
let factor = 10;
console.log(a.map((x: number) => x * factor));
factor = 100;
console.log(a.map((x: number) => x * factor));

// Methods are values, and `a.push === b.push` is true because they are the same function in the
// language -- a method value carries its code and not its receiver.
const b = [7];
console.log(a.push === b.push, a.map === a.map, typeof a.map, a.push, a.map);

// Nested arrays, and an array of objects walked with the callback methods.
const people = [{ name: "a", age: 30 }, { name: "b", age: 20 }];
console.log(people.map((p: { name: string; age: number }) => p.name).join(","));
console.log(people.filter((p: { name: string; age: number }) => p.age > 25));
console.log(people.reduce((total: number, p: { name: string; age: number }) => total + p.age, 0));
console.log([[1, 2], [3]].map((row: number[]) => row.length), [[1], [2]].concat([[3]]));
