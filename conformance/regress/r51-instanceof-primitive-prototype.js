function F() {}
for (const prototype of [undefined, null, false, 1, "x", Symbol("x"), 1n]) {
  F.prototype = prototype;
  let result;
  try { result = {} instanceof F; } catch (error) { result = error.name; }
  console.log(typeof prototype, result, 1 instanceof F);
}
function Target() {}
const Bound = Target.bind(null);
Object.defineProperty(Target, Symbol.hasInstance, {value: function (v) { return v === 7; }});
console.log(7 instanceof Bound, {} instanceof Bound);
const ordinary = Function.prototype[Symbol.hasInstance];
console.log(ordinary.call(Bound, 7), ordinary.call({}, {}));
try { Object.defineProperty(Function.prototype, Symbol.hasInstance, {value: () => true}); } catch (e) { console.log(e.name); }
