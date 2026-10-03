// Indirect eval reaches the native dynamic evaluator through an ordinary call of the intrinsic.
const savedEval = eval;
console.log(savedEval("1 + 2"));
console.log((0, eval)("globalThis.Math.max(7, 9)"));
console.log(globalThis.eval(false));
console.log(savedEval() === undefined);

// Indirect eval sees global bindings and must not capture a caller's lexical environment.
globalThis.__indirectEvalGlobal = 41;
function scopeProbe() {
  const onlyLocal = 23;
  return [savedEval("__indirectEvalGlobal"), savedEval("typeof onlyLocal")];
}
console.log(scopeProbe().join(","));

// A missing global is an ordinary catchable ReferenceError.
let missingNameThrew = false;
try {
  savedEval("__kilnMissingIndirectEvalName__");
} catch (error) {
  missingNameThrew = error instanceof ReferenceError;
}
console.log(missingNameThrew);
let syntaxErrorThrew = false;
try {
  savedEval("(");
} catch (error) {
  syntaxErrorThrew = error instanceof SyntaxError;
}
console.log(syntaxErrorThrew);

// eval returns non-string arguments unchanged without coercing them.
let coercions = 0;
const coercionProbe = { toString() { coercions++; return "1 + 2"; } };
console.log(savedEval(coercionProbe) === coercionProbe, coercions);
