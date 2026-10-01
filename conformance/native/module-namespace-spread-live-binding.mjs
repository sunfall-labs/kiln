import * as source from './fixtures/module-namespace-spread-live.mjs';

const before = { ...source };
console.log(before.current, before.update === source.update);
console.log(Object.keys(before).sort().join(','));
console.log(Object.getOwnPropertyDescriptor(before, Symbol.toStringTag) === undefined);
source.update();
const after = { ...source };
console.log(before.current, after.current);
