// kiln npm functional test: lodash
const _ = require('lodash');

// 1. Core utilities & flattening
console.log(JSON.stringify(_.chunk([1, 2, 3, 4, 5], 2)));
console.log(JSON.stringify(_.uniq([1, 1, 2, 3, 3])));
console.log(JSON.stringify(_.groupBy(['one', 'two', 'three'], 'length')));
console.log(JSON.stringify(_.merge({a: {b: 1}}, {a: {c: 2}})));
console.log(_.capitalize('hello world'));
console.log(JSON.stringify(_.flattenDeep([1, [2, [3]]])));

// 2. Deep equality and cloning
const orig = { a: [1, { b: 2 }], c: 'str' };
const cloned = _.cloneDeep(orig);
cloned.a[1].b = 99;
console.log('clone-isolation:', orig.a[1].b === 2);
console.log('isEqual-equal:', _.isEqual({ x: 1, y: [2, 3] }, { y: [2, 3], x: 1 }));
console.log('isEqual-differ:', _.isEqual({ x: 1 }, { x: '1' }));

// 3. Nested property path get / set / unset
const nested = { users: [{ profile: { name: 'Alice' } }] };
console.log('get-path:', _.get(nested, 'users[0].profile.name'));
console.log('get-default:', _.get(nested, 'users[0].profile.age', 30));
_.set(nested, 'users[0].profile.age', 25);
console.log('set-verify:', nested.users[0].profile.age);
_.set(nested, ['users', 1, 'profile', 'name'], 'Bob');
console.log('set-nested-array:', nested.users.length, nested.users[1].profile.name);
_.unset(nested, 'users[0].profile.age');
console.log('has-after-unset:', _.has(nested, 'users[0].profile.age'));

// 4. Object transforms: pick, omit, mapValues
const obj = { a: 1, b: 2, c: 3 };
console.log('pick:', JSON.stringify(_.pick(obj, ['a', 'c'])));
console.log('omit:', JSON.stringify(_.omit(obj, ['b'])));
console.log('mapValues:', JSON.stringify(_.mapValues(obj, n => n * 10)));

// 5. Collection search, sorting, partitioning with shorthand predicates
const users = [
  { user: 'barney', age: 36, active: false },
  { user: 'fred', age: 40, active: true },
  { user: 'pebbles', age: 1, active: true }
];
console.log('find-shorthand:', JSON.stringify(_.find(users, { age: 1, active: true })));
console.log('orderBy:', JSON.stringify(_.orderBy(users, ['active', 'age'], ['desc', 'asc']).map(u => u.user)));
const [active, inactive] = _.partition(users, 'active');
console.log('partition:', active.length, inactive.length);

// 6. Set operations & functional tools
console.log('difference:', JSON.stringify(_.difference([2, 1, 3], [2, 3])));
console.log('intersection:', JSON.stringify(_.intersection([2, 1], [2, 3])));
const curriedAdd = _.curry((a, b, c) => a + b + c);
console.log('curry:', curriedAdd(1)(2)(3), curriedAdd(1, 2)(3));
const memoized = _.memoize(n => n * 2);
console.log('memoize:', memoized(5), memoized(5));

// 7. Chaining pipeline
const chainRes = _(users)
  .filter('active')
  .sortBy('age')
  .map(u => u.user)
  .value();
console.log('chain:', JSON.stringify(chainRes));
