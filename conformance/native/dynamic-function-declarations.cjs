// Dynamic Function bodies instantiate function declarations before executing statements.
const forward = Function("return later(); function later() { return 17; }");
console.log("forward declaration:", forward());

const mutual = Function("return first(); function first() { return second(); } function second() { return 23; }");
console.log("mutual declaration:", mutual());

const repeated = Function("function choose() { return 1; } function choose() { return 2; } return choose();");
console.log("last declaration wins:", repeated());

const varPreserves = Function("var later; return typeof later; function later() { return 19; }");
console.log("var does not erase declaration:", varPreserves());

const nested = Function(
  "function outer() { return inside(); function inside() { return 31; } } return outer();",
);
console.log("nested declaration:", nested());

const prototypeWrite = Function(
  "function DefaultsError() {} DefaultsError.prototype = Object.create(Error.prototype); " +
    "DefaultsError.prototype.constructor = DefaultsError; " +
    "return DefaultsError.prototype.constructor === DefaultsError;",
);
console.log("declaration prototype:", prototypeWrite());

try {
  Function("undefined.member = 1;")();
  console.log("nullish assignment: no throw");
} catch (error) {
  console.log("nullish assignment:", error.name);
}
