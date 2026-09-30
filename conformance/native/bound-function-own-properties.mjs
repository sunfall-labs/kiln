function predicate() {}
const bound = predicate.bind(null);
bound.numberAllowsNaN = true;
console.log(bound.numberAllowsNaN);
const descriptor = Object.getOwnPropertyDescriptor(bound, "numberAllowsNaN");
console.log(descriptor.value, descriptor.writable, descriptor.enumerable, descriptor.configurable);
