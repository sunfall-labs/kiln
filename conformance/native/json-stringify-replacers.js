const trace = [];
const source = { b: 2, a: 1, remove: 3, child: { a: 4 } };
console.log(JSON.stringify(source, function (key, value) {
  trace.push(key);
  return key === 'remove' ? undefined : typeof value === 'number' ? value * 2 : value;
}));
console.log(trace.join(','));
console.log(JSON.stringify(source, ['a', 'child', 'a'], 2));
console.log(JSON.stringify([1, undefined, 3], (_key, value) => value));
const cyclic = { value: 7 };
cyclic.self = cyclic;
console.log(JSON.stringify(cyclic, (key, value) => key === 'self' ? undefined : value));
try { JSON.stringify(cyclic, (_key, value) => value); }
catch (error) { console.log(error instanceof TypeError); }
let conversions = 0;
const tagged = { toJSON(key) { conversions++; return { key, value: 9 }; } };
console.log(JSON.stringify({ tagged }, (_key, value) => value), conversions);
console.log(JSON.stringify({ ['__proto__']: 2, other: 1 }, ['__proto__']));
