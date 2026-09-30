// kiln eval smoke test (authored for kiln, not from porffor).
const kindOf = require('kind-of');
console.log([kindOf([]), kindOf({}), kindOf('s'), kindOf(1), kindOf(null), kindOf(undefined), kindOf(new Date()), kindOf(/x/g), kindOf(() => {}), kindOf(true), kindOf(Buffer.alloc(1))].join(' '));
console.log(kindOf(new Map()), kindOf(new Set()), kindOf(Symbol('s')), kindOf(10n));
