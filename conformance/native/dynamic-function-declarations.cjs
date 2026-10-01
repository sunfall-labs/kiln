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

const capturedVar = Function(
  "function read() { return later.value; } var later = { value: 29 }; return read();",
);
console.log("declaration captures later var:", capturedVar());

const blockVarCapture = Function(
  "function read() { return inside; } if (true) { var inside = 37; } return read();",
);
console.log("block var capture:", blockVarCapture());

const forVarCapture = Function(
  "function read() { return item; } for (var item = 41; item < 42; item++) {} return read();",
);
console.log("for var capture:", forVarCapture());

const whileVarCapture = Function(
  "function read() { return whileOnly; } var index = 0; " +
    "while (index < 1) { var whileOnly = 45; index++; } return read();",
);
console.log("while var capture:", whileVarCapture());

const whileZeroIterations = Function(
  "var count = 0; while (false) { count = 1; } return count;",
);
console.log("while zero iterations:", whileZeroIterations());

const whileFlow = Function(
  "var count = 0; while (count < 3) { count++; if (count === 1) continue; " +
    "if (count === 2) break; } return count;",
);
console.log("while break and continue:", whileFlow());

const whileReturn = Function("while (true) { return 51; } return 0;");
console.log("while return:", whileReturn());

const labeledWhileBreak = Function("outer: while (true) { break outer; } return 52;");
console.log("labeled while break:", labeledWhileBreak());

const labeledWhileContinue = Function(
  "var count = 0; outer: while (count < 3) { count++; " +
    "while (true) { continue outer; } count = 99; } return count;",
);
console.log("labeled while continue:", labeledWhileContinue());

const nestedVarDoesNotLeak = Function(
  "function inner() { var hidden = 43; } inner(); " +
    "try { return hidden; } catch (error) { return error.name; }",
);
console.log("nested var isolation:", nestedVarDoesNotLeak());

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
