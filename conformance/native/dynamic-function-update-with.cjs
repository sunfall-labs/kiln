// This regression needs sloppy direct eval: `.js` probes run as ESM and inherit strict mode,
// where a `with` statement in the evaluated function expression is a SyntaxError.
function updateWithLexicalShadow() {
  const value = 100;
  const object = { value: 5 };
  const result = eval("(function() { with (object) { return value++; } })()");
  return [result, object.value, value];
}

const result = updateWithLexicalShadow();
if (result[0] !== 5 || result[1] !== 6 || result[2] !== 100) {
  throw new Error("direct eval update must resolve through the sloppy with environment");
}

function supportedExpressions() {
  const input = 3;
  return [
    eval("input + 2"),
    eval("({ answer: input }).answer"),
    eval("(function() { return input; })()"),
    eval(""),
  ];
}
const expressions = supportedExpressions();
if (expressions[0] !== 5 || expressions[1] !== 3 || expressions[2] !== 3 ||
    expressions[3] !== undefined) {
  throw new Error("direct eval returns supported expression values in the caller environment");
}

function shortCircuitPrecedence() {
  let skipped = 0;
  function sideEffect() { skipped++; return true; }
  const andThenConditional = eval(
    "false && sideEffect() ? function() { return 1; } : function() { return 2; }"
  );
  const orThenConditional = eval(
    "true || sideEffect() ? function() { return 3; } : function() { return 4; }"
  );
  const nestedConditional = eval("false ? true ? sideEffect() : sideEffect() : 5");
  const makeFunction = Function("sideEffect", "counter", [
    "var andFn = false && sideEffect() ? function() { return 5; } : function() { return 6; };",
    "var orFn = true || sideEffect() ? function() { return 7; } : function() { return 8; };",
    "var nullishFn = 0 ?? sideEffect() ? function() { return 9; } : function() { return 10; };",
    "var nested = false ? true ? sideEffect() : sideEffect() : 11;",
    "return [andFn(), orFn(), nullishFn(), nested, counter.value];",
  ].join("\n"));
  const counter = { value: 0 };
  function runtimeSideEffect() { counter.value++; return true; }
  const runtimeResult = makeFunction(runtimeSideEffect, counter);
  return [andThenConditional(), orThenConditional(), nestedConditional,
    runtimeResult, skipped];
}
const shortCircuit = shortCircuitPrecedence();
if (shortCircuit[0] !== 2 || shortCircuit[1] !== 3 || shortCircuit[2] !== 5 ||
    shortCircuit[3][0] !== 6 || shortCircuit[3][1] !== 7 || shortCircuit[3][2] !== 10 ||
    shortCircuit[3][3] !== 11 || shortCircuit[3][4] !== 0 || shortCircuit[4] !== 0) {
  throw new Error("short-circuit skipping must preserve conditional precedence and side effects");
}

const parseNumbers = Function("return [0xdc00, 0xDFFF, 0b1010, 0o17, 077, 08, " +
  "1_000, 1.25e2, .5, .5e2, 08.5, 09e2];");
const parsedNumbers = parseNumbers();
if (parsedNumbers.join(",") !== "56320,57343,10,15,63,8,1000,125,0.5,50,8.5,900") {
  throw new Error("dynamic source must parse radix, separated, and exponent number literals");
}
let invalidNumberRejected = false;
try {
  Function("return 0x;");
} catch (error) {
  invalidNumberRejected = error instanceof SyntaxError;
}
if (!invalidNumberRejected) throw new Error("invalid radix number must fail Function construction");
let strictLegacyOctalRejected = false;
try {
  Function('"use strict"; return 08;');
} catch (error) {
  strictLegacyOctalRejected = error instanceof SyntaxError;
}
if (!strictLegacyOctalRejected) throw new Error("strict Function source must reject legacy octal syntax");
for (const body of ["return 00.5;", "return 01e2;", '"use strict"; return 08.5;']) {
  let rejected = false;
  try {
    Function(body);
  } catch (error) {
    rejected = error instanceof SyntaxError;
  }
  if (!rejected) throw new Error("legacy-octal spellings must be rejected in this context");
}
console.log(result.join(","), shortCircuit.join(","), parsedNumbers.join(","));
