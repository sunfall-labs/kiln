// Dynamic Function bodies instantiate function declarations before executing statements.
const forward = Function("return later(); function later() { return 17; }");
console.log("forward declaration:", forward());

const mutual = Function("return first(); function first() { return second(); } function second() { return 23; }");
console.log("mutual declaration:", mutual());

const repeated = Function("function choose() { return 1; } function choose() { return 2; } return choose();");
console.log("last declaration wins:", repeated());

const assignmentBetween = Function(
  "function select() { return 1; } " +
    "select = function () { return 3; }; " +
    "function select() { return 2; } return select();",
);
console.log("declarations do not reinitialize:", assignmentBetween());

const varPreserves = Function("var later; return typeof later; function later() { return 19; }");
console.log("var does not erase declaration:", varPreserves());

const nested = Function(
  "function outer() { return inside(); function inside() { return 31; } } return outer();",
);
console.log("nested declaration:", nested());

const asyncDeclaration = Function(
  "return later().then(value => value + 1); async function later() { return 41; }",
);
asyncDeclaration().then(value => console.log("async declaration:", value));

const stressDeclarations = Array.from({ length: 24 }, (_, index) =>
  `function read${index}() { return seed.value + ${index}; }`,
).join(" ");
const stressCalls = Array.from({ length: 24 }, (_, index) => `read${index}()`).join(" + ");
const manyCapturedDeclarations = Function(
  `var seed = { value: 7 }; ${stressDeclarations} return ${stressCalls};`,
);
console.log("many captured declarations:", manyCapturedDeclarations());

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
