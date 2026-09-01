// `delete` on something that is not a property access. Sloppy code is the only place most of
// this can be written at all -- an unqualified `delete` is an early SyntaxError in strict code --
// which is why the probe is a `.cjs`.
var declared = 1;
console.log('declared:', delete declared, declared);

globalThis.planted = 2;
console.log('planted:', delete planted, typeof planted);

console.log('unresolvable:', delete neverSeenAnywhere);

// The three value properties of the global object are locked on every axis, so `delete` refuses.
console.log('locked:', delete NaN, delete Infinity, delete undefined);

// Not references at all: the operand is evaluated for its effects and the answer is `true`.
var called = false;
function effect() {
  called = true;
  return 0;
}
console.log('non-reference:', delete effect(), called);
console.log('literals:', delete 0, delete 'x', delete void 0, delete delete 0, delete this);
console.log('parenthesised:', delete (declared), delete ({ a: 1 }).a);

// `with` is deliberately absent: the native backend refuses the statement outright, and a probe
// it refuses compares nothing. The interpreter's with-scoped `delete` is covered by Test262's
// `language/statements/with` instead.

function args() {
  return delete arguments;
}
console.log('arguments:', args(1));
