// The other `arguments` object. A sloppy function with a simple parameter list gets the *mapped*
// one, whose `callee` names the function; every other function gets the unmapped one, whose
// `callee` throws. Which of the two a call gets is decided by the compiler -- it takes both the
// strictness and the shape of the parameter list to know -- and travels to each engine on the
// function it belongs to, so this is the gate on those two mechanisms agreeing.
//
// Sloppy code reaches either engine only through a `.cjs` entry, which is why this is not part of
// the `.js` probe beside it.
function simple(a, b) {
  return arguments.callee === simple;
}
console.log("mapped", simple(1, 2));

function withDefault(a, b = 2) {
  try {
    return typeof arguments.callee;
  } catch (error) {
    return error.constructor.name;
  }
}
console.log("default", withDefault(1));

function withRest(...rest) {
  try {
    return typeof arguments.callee;
  } catch (error) {
    return error.constructor.name;
  }
}
console.log("rest", withRest(1));

function alsoStrict() {
  "use strict";
  try {
    return typeof arguments.callee;
  } catch (error) {
    return error.constructor.name;
  }
}
console.log("strict", alsoStrict());

// An arrow sees the enclosing call's object, and so the enclosing call's kind.
function throughArrow(a) {
  const read = () => arguments;
  return [read() === arguments, read().callee === throughArrow].join("|");
}
console.log("arrow", throughArrow(1));
