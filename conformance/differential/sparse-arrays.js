// A hole is not an element whose value is undefined. It is *absent*, and the difference shows in
// `in`, in what enumerates, and in which methods visit it.
const sparse = [1, , 3];
const dense = [1, undefined, 3];
console.log(sparse.length, String(sparse[1]), sparse.join(','), JSON.stringify(sparse));
console.log(1 in sparse, 1 in dense);
console.log(Object.keys(sparse).join(','), Object.keys(dense).join(','));

// Element-walking methods skip a hole and visit an undefined.
const visited = [];
sparse.forEach((v) => visited.push(String(v)));
const denseVisited = [];
dense.forEach((v) => denseVisited.push(String(v)));
console.log(visited.join(','), denseVisited.join(','));
console.log(sparse.filter(() => true).length, dense.filter(() => true).length);

// `for-in` enumerates keys, so it skips holes too; `for-of` walks indices and does not.
const keys = [];
for (const key in sparse) keys.push(key);
const values = [];
for (const value of sparse) values.push(String(value));
console.log(keys.join(','), values.join(','));

// Trailing and leading holes, and several in a row.
console.log([, , 1].length, [1, , ].length, [, ].length);

// A console reports a run of holes rather than repeating them.
console.log([1, , 3]);
console.log([1, , , , 5]);
console.log([, ]);

// Spread and `Array.from` read each index, so a hole becomes undefined.
console.log(JSON.stringify([...sparse]), JSON.stringify(Array.from(sparse)));
