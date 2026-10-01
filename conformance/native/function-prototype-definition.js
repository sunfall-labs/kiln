// Defining a function's intrinsic prototype must update reads, construction and descriptors.
function Factory() {}
const replacement = { marker: 17 };
Object.defineProperty(Factory, 'prototype', { value: replacement });
if (Factory.prototype !== replacement || Object.getPrototypeOf(new Factory()) !== replacement ||
    !Object.getOwnPropertyDescriptor(Factory, 'prototype').writable) {
  throw new Error('value-only prototype definition lost its value or existing writable attribute');
}
const second = { marker: 23 };
Factory.prototype = second;
if (Object.getOwnPropertyDescriptor(Factory, 'prototype').value !== second) {
  throw new Error('prototype assignment must update the descriptor value');
}
Object.defineProperty(Factory, 'prototype', { writable: false });
if (Reflect.set(Factory, 'prototype', replacement) || Factory.prototype !== second ||
    Object.getOwnPropertyDescriptor(Factory, 'prototype').writable) {
  throw new Error('readonly function prototype accepted a write');
}
Object.defineProperty(Factory, 'prototype', { value: second });
for (const descriptor of [{ value: replacement }, { writable: true }, { enumerable: true },
  { configurable: true }, { get() { return replacement; } }]) {
  let rejected = false;
  try { Object.defineProperty(Factory, 'prototype', descriptor); }
  catch (error) { rejected = error instanceof TypeError; }
  if (!rejected) throw new Error('intrinsic prototype reconfiguration must throw TypeError');
}
function Sealed() {}
Object.preventExtensions(Sealed);
Object.defineProperty(Sealed, 'prototype', { value: replacement });
if (Sealed.prototype !== replacement) throw new Error('non-extensibility must preserve existing prototype definitions');
const arrow = () => {};
arrow.prototype = replacement;
const arrowDescriptor = Object.getOwnPropertyDescriptor(arrow, 'prototype');
if (arrow.prototype !== replacement || !arrowDescriptor.writable || !arrowDescriptor.enumerable ||
    !arrowDescriptor.configurable) throw new Error('an arrow prototype property is an ordinary own property');
console.log('function prototype definitions: ok');
