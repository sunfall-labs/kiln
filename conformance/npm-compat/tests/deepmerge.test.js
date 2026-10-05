const deepmerge = require('deepmerge');

console.log('--- Nested Object & Array Merging ---');
const obj1 = {
  user: { name: 'Alice', details: { age: 30, city: 'Denver' } },
  tags: ['admin'],
  flags: { active: true }
};
const obj2 = {
  user: { details: { city: 'Boulder', zip: 80301 } },
  tags: ['developer', 'runner'],
  flags: { verified: true }
};
const merged = deepmerge(obj1, obj2);
console.log(JSON.stringify(merged));

console.log('--- Input immutability (cloning) ---');
console.log('obj1 not mutated:', obj1.user.details.city === 'Denver');
console.log('merged.user !== obj1.user:', merged.user !== obj1.user);
console.log('merged.tags !== obj1.tags:', merged.tags !== obj1.tags);

console.log('--- Custom arrayMerge option ---');
const overwriteMerge = (destinationArray, sourceArray, options) => sourceArray;
const customArr = deepmerge({ items: [1, 2] }, { items: [3, 4] }, { arrayMerge: overwriteMerge });
console.log(JSON.stringify(customArr));

console.log('--- deepmerge.all with multiple objects ---');
const o1 = { a: 1, shared: { x: 1 } };
const o2 = { b: 2, shared: { y: 2 } };
const o3 = { c: 3, shared: { z: 3 } };
const allMerged = deepmerge.all([o1, o2, o3]);
console.log(JSON.stringify(allMerged));

console.log('--- Boundary / Edge cases ---');
console.log(JSON.stringify(deepmerge({}, {})));
console.log(JSON.stringify(deepmerge({ val: null }, { val: 5 })));
console.log(JSON.stringify(deepmerge({ val: 5 }, { val: null })));
console.log(JSON.stringify(deepmerge({ val: undefined }, { val: 'set' })));
