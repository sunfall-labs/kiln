const regexp = /x/;
console.log(Object.prototype.toString.call(regexp));
console.log(RegExp.prototype[Symbol.toStringTag]);
console.log(RegExp.prototype.exec.length, RegExp.prototype.test.length,
  RegExp.prototype.toString.length);
console.log(Object.keys(regexp).join(","));
console.log(Object.getOwnPropertyNames(regexp).join(","));
const descriptor = Object.getOwnPropertyDescriptor(regexp, "lastIndex");
console.log(descriptor.value, descriptor.writable, descriptor.enumerable,
  descriptor.configurable);
