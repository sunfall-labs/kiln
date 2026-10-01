// A named function expression owns an immutable inner binding. Sloppy writes are ignored,
// strict writes throw, and either binding shadows an outer variable with the same name.
function factory(source) { return eval(source); }
let shadow = 'outer';
const sloppy = factory('(function shadow() { shadow = 3; return shadow; })');
console.log(sloppy() === sloppy, shadow);

let strictName = 'outer strict';
const strict = factory('(function strictName() { "use strict"; strictName = 4; })');
try {
  strict();
} catch (error) {
  console.log(error instanceof TypeError, strictName);
}

const recursive = factory('(function factorial(n) { return n < 2 ? 1 : n * factorial(n - 1); })');
console.log(recursive(5));
