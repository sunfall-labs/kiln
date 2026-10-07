function NewTarget() {}
NewTarget.prototype = 1;
const result = Reflect.construct(SuppressedError, [{}, {}], NewTarget);
console.log(
  Object.getPrototypeOf(result) === SuppressedError.prototype,
  Object.getPrototypeOf(result) === Object.prototype,
);
