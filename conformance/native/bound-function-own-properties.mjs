function predicate() {}
const bound = predicate.bind(null);
bound.numberAllowsNaN = true;
console.log(bound.numberAllowsNaN);
const descriptor = Object.getOwnPropertyDescriptor(bound, "numberAllowsNaN");
console.log(descriptor.value, descriptor.writable, descriptor.enumerable, descriptor.configurable);

if (bound.absentFeature !== undefined) throw new Error("missing bound feature is not absent");
const inherited = { inheritedFeature: 17 };
Object.setPrototypeOf(bound, inherited);
if (bound.inheritedFeature !== 17) throw new Error("bound function lost its explicit prototype");
Object.setPrototypeOf(bound, null);
if (bound.absentFeature !== undefined) throw new Error("null bound prototype is not terminal");
