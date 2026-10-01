const rounded = [
  Math.f16round(1.00048828125),
  Math.f16round(1.0006),
  Object.is(Math.f16round(-0), -0),
  Number.isNaN(Math.f16round()),
];
console.log(...rounded);

const nested = [];
const outer = Math.f16round({ valueOf() {
  nested.push(Math.f16round(1.0006));
  return 1.00048828125;
} });
console.log(outer, nested[0]);

const coercions = [];
const bits = { valueOf() { coercions.push("bits"); return 8.9; } };
const value = { [Symbol.toPrimitive](hint) { coercions.push(`value:${hint}`); return -1n; } };
console.log(BigInt.asIntN(bits, value), BigInt.asUintN(8, -1n),
  BigInt.asIntN(8, "255"), BigInt.asUintN(8, true), coercions.join(","));

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
  BigInt.asIntN(Number.MAX_SAFE_INTEGER, 1n),
  BigInt.asUintN(Number.MAX_SAFE_INTEGER, 1n));

const rejectedNumbers = [];
for (const input of [1, new Number(1), { valueOf() { return 1; } }]) {
  try {
    BigInt.asIntN(8, input);
    rejectedNumbers.push(false);
  } catch (error) {
    rejectedNumbers.push(error instanceof TypeError);
  }
}
const zeroOrder = [];
const zero = BigInt.asIntN(
  { valueOf() { zeroOrder.push("bits"); return 0; } },
  { [Symbol.toPrimitive](hint) { zeroOrder.push(`value:${hint}`); return 2n; } },
);
console.log(...rejectedNumbers, zero, zeroOrder.join(","));
console.log(BigInt.asIntN(8, {
  [Symbol.toPrimitive]: null,
  valueOf() { return 12n; },
}));
