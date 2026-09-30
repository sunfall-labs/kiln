// kiln eval smoke test (authored for kiln, not from porffor).
const { produce } = require('immer');
const base = { count: 0, items: [] };
const next = produce(base, (d) => { d.count = 1; d.items.push('x'); });
console.log(JSON.stringify(next));
console.log(base.count === 0 && next !== base);
const frozen = produce({ a: { b: 2 } }, (d) => { d.a.b = 3; });
console.log(JSON.stringify(frozen));
console.log(Object.isFrozen(frozen));
const unchanged = produce(base, () => {});
console.log(unchanged === base);
