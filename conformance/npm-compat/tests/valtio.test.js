// kiln eval smoke test (authored for kiln, not from porffor).
const {proxy, snapshot} = require('valtio');
const state = proxy({count: 0, nested: {x: 1}});
state.count = 5;
console.log(snapshot(state).count);
console.log(JSON.stringify(snapshot(state).nested));
state.nested.x = 9;
console.log(snapshot(state).nested.x);
const s2 = proxy({a: 1});
s2.a = 2;
console.log(snapshot(s2).a);
console.log(typeof proxy, typeof snapshot);
