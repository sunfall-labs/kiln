// Object's complete static inventory proves unknown own keys absent, while its store takes priority.
console.log(Object.getOwnPropertyNames(Object).sort().join(','));
for (const key of ['children', 'toString', 'constructor', 'prototype', 'hasOwn', 'name', 'length', Symbol.iterator]) {
  console.log(String(key), Object.hasOwn(Object, key));
}
const key = Symbol('own');
Object.children = 7;
Object[key] = 8;
console.log(Object.hasOwn(Object, 'children'), Object.hasOwn(Object, key));
delete Object.children;
delete Object[key];
console.log(Object.hasOwn(Object, 'children'), Object.hasOwn(Object, key));
