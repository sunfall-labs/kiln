// g01 checks that spread and rest carry symbol keys across. What decides whether an object
// literal is *correct* is the ordering rule: a spread contributes what its source has at the
// point it appears, so a property after it wins and one before it loses.
const base = { a: 1, b: 2 };
console.log(JSON.stringify({ a: 0, ...base, b: 9 }));
console.log(JSON.stringify({ ...base, ...{ b: 3, c: 4 } }));

// Spreading nothing contributes nothing rather than throwing, which is where object spread is
// more forgiving than array spread.
console.log(JSON.stringify({ ...null, ...undefined, ok: true }));

// Anything with own indexed properties contributes them.
console.log(JSON.stringify({ ...[7, 8] }), JSON.stringify({ ...'hi' }));

// A computed key is an arbitrary expression and is evaluated in source order, between the
// members around it -- not hoisted to the start or deferred to the end.
const order = [];
const key = (name) => { order.push(name); return name; };
console.log(JSON.stringify({ [key('x')]: 1, ...{ y: 2 }, [key('z')]: 3 }), order.join(','));

// Rest takes what the named properties did not, including a renamed one and a missing one.
const { a, ...rest } = { a: 1, b: 2, c: 3 };
const { b: renamed, ...others } = { b: 5, d: 6 };
const { absent, ...all } = base;
console.log(a, JSON.stringify(rest), renamed, JSON.stringify(others), String(absent), JSON.stringify(all));

// Two symbols with the same description are different properties.
const first = Symbol('same');
const second = Symbol('same');
const tagged = { [first]: 'one', [second]: 'two' };
console.log(tagged[first], tagged[second], first === second, Object.keys(tagged).length);
