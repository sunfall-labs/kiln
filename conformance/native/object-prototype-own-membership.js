const hasOwn = Object.prototype.hasOwnProperty;
const standard = [
  'constructor',
  'hasOwnProperty',
  'isPrototypeOf',
  'propertyIsEnumerable',
  'toLocaleString',
  'toString',
  'valueOf',
  '__proto__',
  '__defineGetter__',
  '__defineSetter__',
  '__lookupGetter__',
  '__lookupSetter__',
];
const results = [];
for (const key of standard) results.push(hasOwn.call(Object.prototype, key));
console.log(results.join(','));

console.log(
  hasOwn.call(Object.prototype, 'not-an-own-member'),
  hasOwn.call(Object.prototype, '🧪'),
  hasOwn.call(Object.prototype, Symbol('not-an-own-member')),
);

Object.prototype.__kilnOwnProbe = 1;
const descendant = Object.create(Object.prototype);
console.log(
  hasOwn.call(Object.prototype, '__kilnOwnProbe'),
  hasOwn.call(descendant, '__kilnOwnProbe'),
);
delete Object.prototype.__kilnOwnProbe;
console.log(hasOwn.call(Object.prototype, '__kilnOwnProbe'));

const inherited = Object.create({ inheritedOnly: true });
console.log(hasOwn.call(inherited, 'inheritedOnly'), hasOwn.call(inherited, 'toString'));
