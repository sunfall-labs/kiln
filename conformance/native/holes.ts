// An array slot that was never written, or was deleted, is a *hole*. It reads as undefined,
// which is why the marker must never escape into an expression -- but it is not one:
// `1 in [1, , 3]` is false, a console shows `<1 empty item>`, and every element-walking method
// skips it. This is also the smallest program that hands a reference to the host, which is how
// it caught a boundary conversion that was only emitted for programs making a callable.
console.log([1]);
console.log([1, , 3], [1, , 3].length, 1 in [1, , 3], [1, , 3][1]);
console.log([, , ,], [1, , , 4], [,].length);

const deleted: any[] = [1, 2, 3];
delete deleted[1];
console.log(deleted, 1 in deleted, deleted.length, deleted[1], 0 in deleted);
console.log(Object.keys(deleted), JSON.stringify(deleted), deleted.join("-"), `${deleted}`);

// A hole and an undefined element are different things that read the same.
const explicit: any[] = [1, undefined, 3];
console.log(explicit, 1 in explicit, explicit[1] === deleted[1], Object.keys(explicit).length);

// The walks: `map` leaves a hole where it found one, because its result has an entry per input
// position and skipping would shift everything after it. `filter` drops it. Neither calls back.
let calls = 0;
console.log(deleted.map((x: number) => { calls += 1; return x; }), calls);
console.log(deleted.filter(() => true), calls, deleted.reduce((t: number, x: number) => t + x, 0));

// `indexOf` skips a hole and `includes` sees the undefined it behaves like -- the only place
// those two disagree, and the reason `includes` exists.
console.log(deleted.indexOf(undefined), deleted.includes(undefined), explicit.indexOf(undefined));

// Writing into a hole fills it, and growing past the end leaves more.
deleted[1] = 9;
console.log(deleted, 1 in deleted);
const grown: any[] = [1];
grown[3] = 4;
console.log(grown, grown.length, 1 in grown, JSON.stringify(grown));

// `for...in` visits only the positions that are there.
const visited: string[] = [];
for (const index in grown) { visited.push(index); }
console.log(visited);

// `for...of` visits every position, including a hole, which reads as undefined -- unlike
// `forEach`, which skips one. Spread walks the array directly and `for...of` drives the
// iteration protocol, so both paths have to agree about that.
const walked: any[] = [];
for (const value of [1, , 3]) { walked.push(value); }
console.log(walked, [...[1, , 3]], [1, , 3].length);
let visits = 0;
[1, , 3].forEach(() => { visits += 1; });
console.log(visits, [...[1, , 3].values()]);
