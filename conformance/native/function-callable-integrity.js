function extensibleTarget(a, b) {}
Object.preventExtensions(extensibleTarget);
const nonExtensibleName = Object.getOwnPropertyDescriptor(extensibleTarget, "name");
const nonExtensibleLength = Object.getOwnPropertyDescriptor(extensibleTarget, "length");
if (
  Object.isExtensible(extensibleTarget) ||
  !nonExtensibleName.configurable ||
  !nonExtensibleLength.configurable
) {
  throw new Error("preventExtensions must preserve callable name and length descriptors");
}
Object.defineProperty(extensibleTarget, "name", { value: "renamed" });
if (extensibleTarget.name !== "renamed") {
  throw new Error("preventExtensions must still allow redefining an existing callable property");
}

function sealedTarget(a) {}
Object.seal(sealedTarget);
if (
  !Object.isSealed(sealedTarget) ||
  Object.getOwnPropertyDescriptor(sealedTarget, "name").configurable ||
  Object.getOwnPropertyDescriptor(sealedTarget, "length").configurable
) {
  throw new Error("seal must make callable name and length non-configurable");
}

function frozenTarget(a, b, c) {}
Object.freeze(frozenTarget);
const frozenName = Object.getOwnPropertyDescriptor(frozenTarget, "name");
const frozenLength = Object.getOwnPropertyDescriptor(frozenTarget, "length");
if (
  !Object.isFrozen(frozenTarget) ||
  frozenName.configurable ||
  frozenName.writable ||
  frozenLength.configurable ||
  frozenLength.writable ||
  Object.getOwnPropertyDescriptor(frozenTarget, "prototype").writable
) {
  throw new Error("freeze must update all callable intrinsic descriptors");
}

console.log(
  nonExtensibleName.configurable,
  nonExtensibleLength.configurable,
  Object.getOwnPropertyDescriptor(sealedTarget, "name").configurable,
  Object.getOwnPropertyDescriptor(frozenTarget, "name").configurable,
  Object.getOwnPropertyDescriptor(frozenTarget, "prototype").writable,
);
