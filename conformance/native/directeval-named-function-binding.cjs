// A named function expression owns an immutable inner binding. Sloppy writes are ignored,
// strict writes throw, and either binding shadows an outer variable with the same name.
function factory(source) { return eval(source); }
let shadow = 'outer';
const staticSloppy = function staticName() { staticName = 3; return staticName; };
console.log(staticSloppy() === staticSloppy);
if (staticSloppy() !== staticSloppy) throw new Error('sloppy static function-name write changed binding');
let strictStaticWriteThrew = false;
try {
  (function strictName() { 'use strict'; strictName = 4; })();
} catch (error) {
  strictStaticWriteThrew = error instanceof TypeError;
}
if (!strictStaticWriteThrew) throw new Error('strict static function-name write did not throw');
const sloppy = factory('(function shadow() { shadow = 3; return shadow; })');
console.log(sloppy() === sloppy, shadow);
if (sloppy() !== sloppy || shadow !== 'outer') throw new Error('sloppy function-name write changed binding');

let strictName = 'outer strict';
const strict = factory('(function strictName() { "use strict"; strictName = 4; })');
try {
  strict();
} catch (error) {
  console.log(error instanceof TypeError, strictName);
}
let strictWriteThrew = false;
try {
  strict();
} catch (error) {
  strictWriteThrew = error instanceof TypeError;
}
if (!strictWriteThrew || strictName !== 'outer strict') {
  throw new Error('strict function-name write did not preserve its bindings');
}

const recursive = factory('(function factorial(n) { return n < 2 ? 1 : n * factorial(n - 1); })');
console.log(recursive(5));

function captureInheritedNames(source) {
  const constructor = 'lexical-constructor';
  const toString = 'lexical-toString';
  const valueOf = 'lexical-valueOf';
  const hasOwnProperty = 'lexical-hasOwnProperty';
  const __proto__ = 'lexical-proto';
  return eval(source);
}
console.log(captureInheritedNames(
  '(function () { return [constructor, toString, valueOf, hasOwnProperty, __proto__].join("|"); })',
)());

function namedConstructor(source) {
  let constructor = 'outer-constructor';
  const inner = eval(source);
  return [inner, inner(), constructor];
}
const named = namedConstructor('(function constructor() { constructor = 1; return constructor; })');
console.log(named[0] === named[1], named[2]);
if (named[0] !== named[1] || named[2] !== 'outer-constructor') {
  throw new Error('named constructor self binding was changed');
}
