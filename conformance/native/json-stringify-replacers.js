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
const revived = JSON.parse('{"a":1,"gone":2,"items":[3,4]}', function (key, value) {
  return key === 'gone' ? undefined : typeof value === 'number' ? value + 10 : value;
});
console.log(JSON.stringify(revived));
console.log(JSON.parse('4', (_key, value) => value * 3));
const tokens = [];
JSON.parse('{"n":1e2,"s":"\\u0061","a":[true,null]}', (_key, value, context) => {
  if (context.source !== undefined) tokens.push(context.source);
  return value;
});
console.log(tokens.join('|'));
console.log(JSON.parse('{"a":{"old":1},"a":null}', (_key, value) => value).a === null);

console.log(JSON.parse('prefix{"a":2}suffix'.slice(6, -6)).a);
console.log(JSON.parse('prefix"\u0100"suffix'.slice(6, -6)));
