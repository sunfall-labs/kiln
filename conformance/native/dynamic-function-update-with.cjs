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
  return [andThenConditional(), orThenConditional(), nestedConditional, skipped];
}
const shortCircuit = shortCircuitPrecedence();
if (shortCircuit[0] !== 2 || shortCircuit[1] !== 3 || shortCircuit[2] !== 5 ||
    shortCircuit[3] !== 0) {
  throw new Error("short-circuit skipping must preserve conditional precedence and side effects");
}
console.log(result.join(","), shortCircuit.join(","));
