function F() {}
const first = Object.create(Function.prototype);
first.tag = "captured";
Object.setPrototypeOf(F, first);
const bound = F.bind(null);
const twice = bound.bind(null);
Object.setPrototypeOf(F, Function.prototype);
console.log(Object.getPrototypeOf(bound) === first, Object.getPrototypeOf(twice) === first, bound.tag, twice.tag);
Object.setPrototypeOf(bound, Function.prototype);
console.log(Object.getPrototypeOf(bound) === Function.prototype, Object.getPrototypeOf(twice) === first);
function G() {}
const bind = G.bind; Object.setPrototypeOf(G, null);
const nullBound = bind.call(G, null);
console.log(Object.getPrototypeOf(nullBound) === null);
