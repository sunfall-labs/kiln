// `?.` does not short-circuit only its own access: in `a?.b.c` a nullish `a` makes the *whole*
// chain undefined rather than reading `.c` of undefined.
const present = { b: { c: 1 }, list: [10, 20], fn() { return 'called'; } };
const absent = null;
console.log(present?.b.c, absent?.b.c, present?.b?.c, absent?.b?.c);
console.log(present?.list[1], absent?.list[1], present?.['list']?.[0], absent?.['list'][0]);
console.log(present?.fn(), absent?.fn(), present.missing?.deep.deeper);

// Undefined short-circuits like null, and nothing else does -- 0 and "" are not nullish.
const zero = { at: 0 };
console.log(undefined?.x, zero.at?.toString(), (0)?.toString(), ''?.length);

// The chain ends at its own boundary: a `?.` inside an argument belongs to its own chain, and a
// short circuit inside one does not escape into the call around it.
const wrap = (value) => 'wrapped:' + value;
console.log(wrap(absent?.b), present?.fn(absent?.b));

// The object is evaluated once even when the access short-circuits.
const evaluated = [];
const probe = (value) => { evaluated.push('probe'); return value; };
console.log(probe(absent)?.anything, probe(present)?.b.c, evaluated.length);

// Short-circuiting skips the property expression entirely.
const keys = [];
const key = (name) => { keys.push(name); return name; };
console.log(absent?.[key('skipped')], present?.[key('b')].c, keys.join(','));
