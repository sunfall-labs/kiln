// The operators a real bundle turned out to need. Logical assignment is not a binary operator
// with a store wrapped round it: it *tests* the current value and assigns only if it is the wrong
// one, so the right-hand side may never run and the store may never happen.
let evaluated = 0;
const rhs = (v) => { evaluated++; return v; };
let a = null; a ??= rhs('set');
let b = 1; b ??= rhs('skipped');
let c = 0; c ||= rhs(9);
let d = 1; d &&= rhs(5);
let e = 0; e &&= rhs('skipped');
console.log(a, b, c, d, e, evaluated);

// The same on a property, where the receiver must not be evaluated twice either.
let calls = 0;
const target = { present: 1, absent: null };
const receiver = () => { calls++; return target; };
console.log(receiver().absent ??= 'filled', receiver().present ??= 'skipped', calls);
const key = 'absent';
console.log(target[key] ||= 'again', target[key]);

// Bitwise compound assignment.
let n = 6; n &= 3;
let m = 6; m |= 1;
let x = 6; x ^= 3;
let l = 1; l <<= 3;
let r = -8; r >>= 1;
let u = -8; u >>>= 28;
console.log(n, m, x, l, r, u);

// Updating a property: the object and the key are each evaluated once, and a postfix update
// yields the old value *as a number*.
const counter = { n: 1, s: '5' };
console.log(counter.n++, counter.n, ++counter.n, counter.n--, counter.n);
console.log(counter.s++, counter.s);
const list = [5];
let index = 0;
const at = () => { index++; return 0; };
console.log(list[at()]++, list[0], index);

// Destructuring in a for-of head, which is how anything walks entries.
for (const [k, v] of [['a', 1], ['b', 2]]) console.log(k, v);
for (const { id, tag = 'none' } of [{ id: 7 }, { id: 8, tag: 'set' }]) console.log(id, tag);
for (const [first, ...rest] of [[1, 2, 3]]) console.log(first, rest.join(','));

// An update converts to a number first: `x++` where `x` is "5" is 5 then 6, not "5" then "51".
let text = '5';
console.log(text++, text);
let counted = '3';
console.log(--counted, counted);
let notNumeric = 'x';
console.log(notNumeric++, notNumeric);
