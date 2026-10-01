const rounded = [
  Math.f16round(1.00048828125),
  Math.f16round(1.0006),
  Object.is(Math.f16round(-0), -0),
  Number.isNaN(Math.f16round()),
];
console.log(...rounded);

const coercions = [];
const bits = { valueOf() { coercions.push("bits"); return 8.9; } };
const value = { [Symbol.toPrimitive](hint) { coercions.push(`value:${hint}`); return -1n; } };
console.log(BigInt.asIntN(bits, value), BigInt.asUintN(8, -1n), coercions.join(","));

const order = [];
try {
  BigInt.asIntN({ valueOf() { order.push("bits"); return -1; } }, {
    [Symbol.toPrimitive]() { order.push("value"); return 1n; },
  });
} catch (error) {
  console.log(error instanceof RangeError, order.join(","));
}

const descriptor = Object.getOwnPropertyDescriptor(BigInt, "asUintN");
console.log(descriptor.writable, descriptor.enumerable, descriptor.configurable,
  BigInt.asIntN(Number.MAX_SAFE_INTEGER, 1n));
