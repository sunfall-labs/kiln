function sample(first, second) {}

const originalName = Object.getOwnPropertyDescriptor(sample, "name");
sample.name = "ignored";
const reflectedWrite = Reflect.set(sample, "length", 7);
if (sample.name !== "sample" || sample.length !== 2 || reflectedWrite) {
  throw new Error("implicit callable name and length descriptors must stay read-only");
}
let strictWriteRejected = false;
try {
  (function () {
    "use strict";
    sample.name = "ignored";
  })();
} catch (error) {
  strictWriteRejected = error instanceof TypeError;
}
if (!strictWriteRejected) {
  throw new Error("strict assignment to callable name must throw");
}
const builtinMethod = Math.max;
builtinMethod.name = "ignored";
if (builtinMethod.name !== "max" || Reflect.set(builtinMethod, "length", 9)) {
  throw new Error("native callable name and length descriptors must stay read-only");
}

Object.defineProperty(sample, "name", { value: "renamed" });
let descriptor = Object.getOwnPropertyDescriptor(sample, "name");
if (
  descriptor.value !== "renamed" ||
  descriptor.writable !== false ||
  descriptor.enumerable !== false ||
  descriptor.configurable !== true
) {
  throw new Error("value-only definition must preserve callable name attributes");
}
Object.defineProperty(sample, "name", { value: "renamed-again" });
if (sample.name !== "renamed-again") {
  throw new Error("configurable callable name must allow redefinition");
}

Object.defineProperty(sample, "length", { value: 5 });
descriptor = Object.getOwnPropertyDescriptor(sample, "length");
if (
  descriptor.value !== 5 ||
  descriptor.writable !== false ||
  descriptor.enumerable !== false ||
  descriptor.configurable !== true
) {
  throw new Error("value-only definition must preserve callable length attributes");
}

console.log(originalName.value, descriptor.value, descriptor.writable, descriptor.enumerable, descriptor.configurable);
