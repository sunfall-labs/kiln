function Target(value) {
  this.value = value;
  this.seenNewTarget = new.target;
}

function Derived() {}
Derived.prototype.kind = "derived";

const instance = Reflect.construct(Target, ["value"], Derived);
console.log(
  instance.value,
  instance.seenNewTarget === Derived,
  Object.getPrototypeOf(instance) === Derived.prototype,
  instance.kind,
);

const array = Reflect.construct(Array, [1, 2], Derived);
console.log(Array.isArray(array), array.length, array[1], Object.getPrototypeOf(array) === Derived.prototype);

let argumentRead = false;
try {
  Reflect.construct({}, { get length() { argumentRead = true; return 0; } }, Derived);
} catch (error) {
  console.log(error instanceof TypeError, argumentRead);
}

for (const invalid of [Symbol, BigInt, () => {}]) {
  try {
    Reflect.construct(Target, [], invalid);
  } catch (error) {
    console.log(error instanceof TypeError);
  }
}
