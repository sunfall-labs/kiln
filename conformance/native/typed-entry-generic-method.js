// A directly called leaf gets a typed entry in release builds. A method without a specialized
// intrinsic must still take that entry's generic dispatch path instead of refusing the call.
const writable = () => {
  const descriptor = Object.getOwnPropertyDescriptor(Error, "stackTraceLimit");
  return descriptor === undefined
    ? Object.isExtensible(Error)
    : Object.hasOwn(descriptor, "writable");
};

console.log(writable());
