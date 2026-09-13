// ECMA-262 §23.1.3.32: Array.prototype [ @@unscopables ]
//
// The initial value of Array.prototype[Symbol.unscopables] is an object created
// with a null prototype and 16 properties (all set to true):
// at, copyWithin, entries, fill, find, findIndex, findLast, findLastIndex,
// flat, flatMap, includes, keys, toReversed, toSorted, toSpliced, values.
//
// Descriptor attributes on Array.prototype: { writable: false, enumerable: false, configurable: true }.
// Identity is stable across reads.

const unscopables = Array.prototype[Symbol.unscopables];

// 1. Identity across reads and via array instance
console.log('identity:', unscopables === Array.prototype[Symbol.unscopables]);
console.log('instance:', [][Symbol.unscopables] === unscopables);

// 2. Prototype of unscopables object must be null
console.log('prototype:', Object.getPrototypeOf(unscopables) === null);

// 3. 'in' operator checks
console.log('in-proto:', Symbol.unscopables in Array.prototype);
console.log('in-instance:', Symbol.unscopables in []);

// 4. Descriptor on Array.prototype
const desc = Object.getOwnPropertyDescriptor(Array.prototype, Symbol.unscopables);
console.log('desc-value:', desc.value === unscopables);
console.log('desc-attrs:', desc.writable, desc.enumerable, desc.configurable);

// 5. Symbol presence in own property symbols
const symbols = Object.getOwnPropertySymbols(Array.prototype).map(String).sort();
console.log('symbols:', symbols.includes('Symbol(Symbol.unscopables)'));

// 6. Properties on unscopables object
const keys = Object.keys(unscopables).sort();
console.log('keys-count:', keys.length);
console.log('keys-list:', keys.join(','));
console.log('all-true:', keys.every(k => unscopables[k] === true));

// 7. Individual property descriptor checks on the unscopables object
const atDesc = Object.getOwnPropertyDescriptor(unscopables, 'at');
console.log('prop-desc:', atDesc.value, atDesc.writable, atDesc.enumerable, atDesc.configurable);
