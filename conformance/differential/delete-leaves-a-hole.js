// Removing an element leaves a *hole*, not an undefined: the slot reads as undefined and
// `length` is unchanged, but `1 in a` is false, a console shows `<1 empty item>`, and every
// element-walking method skips it. kiln wrote undefined -- the comment beside it said hole,
// which is how the difference survived: the code and its own comment disagreed.
const deleted = [1, 2, 3];
delete deleted[1];
console.log(deleted, deleted.length, 1 in deleted, deleted[1], 0 in deleted);
console.log(Object.keys(deleted), JSON.stringify(deleted), deleted.join("-"));

// A literal hole and a deleted one are the same thing.
const literal = [1, , 3];
console.log(literal, 1 in literal, literal.length, JSON.stringify(literal));
console.log(deleted.length === literal.length, 1 in deleted === (1 in literal));

// The walks: `map` leaves a hole where it found one, because its result has an entry per input
// position and skipping would shift everything after it. `filter` drops it. Neither calls back.
let calls = 0;
console.log(deleted.map((x) => { calls += 1; return x; }), calls);
console.log(deleted.filter(() => true), deleted.forEach(() => {}), calls);
console.log(deleted.indexOf(undefined), deleted.includes(undefined));

// Writing into the hole fills it.
deleted[1] = 9;
console.log(deleted, 1 in deleted, deleted.length);

// `indexOf` skips a hole and `includes` sees it as the undefined it behaves like everywhere else.
// That is the only place the two disagree, and it is the reason `includes` exists.
console.log([1, , 3].indexOf(undefined), [1, , 3].includes(undefined));
console.log([NaN].indexOf(NaN), [NaN].includes(NaN), [-0].indexOf(0), [-0].includes(0));
