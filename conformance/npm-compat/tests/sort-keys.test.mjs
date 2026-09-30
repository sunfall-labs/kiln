// kiln eval smoke test (authored for kiln, not from porffor).
import sortKeys from 'sort-keys';
console.log(JSON.stringify(sortKeys({b: 1, a: 2, c: 3})));
console.log(JSON.stringify(sortKeys({b: {d: 1, c: 2}, a: 1}, {deep: true})));
console.log(JSON.stringify(Object.keys(sortKeys({z: 1, m: 2, a: 3}))));
console.log(JSON.stringify(sortKeys({})));
