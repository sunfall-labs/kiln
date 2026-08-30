// Symbol keys are enumerable own properties, but remain absent from the string-only enumeration
// APIs. Runtime bookkeeping uses the same reserved spelling internally, so this probe also checks
// that copying a symbol property does not turn it into an internal slot.
const first = Symbol("first");
const second = Symbol("second");
let reads = 0;
const source: any = {
  plain: 1,
  [first]: 2,
  get [second](): number { reads += 1; return 3; },
};

const spread: any = { ...source };
const rest: any = (({ plain, ...tail }) => tail)(source);
const assigned: any = Object.assign({}, source);
console.log(spread[first], spread[second], rest[first], rest[second], assigned[first], assigned[second], reads);
console.log(Object.keys(source), Object.values(source), Object.entries(source));
console.log(JSON.stringify(source), JSON.stringify(spread), JSON.stringify(assigned));
console.log(Object.hasOwn(source, first), Object.prototype.hasOwnProperty.call(source, second),
  Object.prototype.propertyIsEnumerable.call(source, first));

console.log(first in source, second in source, "plain" in source, "missing" in source);
console.log(delete source[first], first in source, source[first]);
console.log(delete source[second], second in source, source[second], reads);

const array: any[] = [];
const arraySymbol = Symbol("array");
array[arraySymbol] = 4;
array["@@iterator"] = 5;
array.values = 6;
array.constructor = 7;
console.log(array[arraySymbol], Object.hasOwn(array, arraySymbol),
  Object.prototype.hasOwnProperty.call(array, arraySymbol),
  Object.prototype.propertyIsEnumerable.call(array, arraySymbol),
  Object.getOwnPropertySymbols(array).length);
console.log(array["@@iterator"], typeof array[Symbol.iterator], array.values, array.constructor);
