import { runInNewContext } from "node:vm";

const result = runInNewContext("let n = x + 1; n * 2", { x: 4 });
if (result !== 10) {
  throw new Error(`new-context completion was ${result}, expected 10`);
}
const state = { value: 0 };
const sideEffectResult = runInNewContext(
  "let local = state.value + 2; state.value = local; local + 3",
  { state },
);
if (sideEffectResult !== 5 || state.value !== 2) {
  throw new Error(`new-context eval lost completion or side effect: ${sideEffectResult}, ${state.value}`);
}
const context = { x: 4 };
if (runInNewContext("x = 7", context) !== 7 || context.x !== 7) {
  throw new Error(`new-context assignment did not update its binding: ${context.x}`);
}
const shadowedContext = { x: 4 };
const shadowResult = runInNewContext("let x = 8; x", shadowedContext);
if (shadowResult !== 8 || shadowedContext.x !== 4) {
  throw new Error(`new-context lexical declaration did not shadow its object binding: ${shadowResult}, ${shadowedContext.x}`);
}
let setterCalls = 0;
let accessorValue = 4;
const accessorContext = {
  get x() { return accessorValue; },
  set x(value) { setterCalls++; accessorValue = value; },
};
if (runInNewContext("x = 9; x", accessorContext) !== 9 ||
    accessorValue !== 9 || setterCalls !== 1) {
  throw new Error("new-context assignment did not preserve context accessor semantics");
}
if (runInNewContext('"use strict"; x = 10; x', accessorContext) !== 10 ||
    accessorValue !== 10 || setterCalls !== 2) {
  throw new Error("strict new-context eval lost its object environment");
}
const strictEvalContext = {};
if (runInNewContext('"use strict"; let evalLocal = 11; evalLocal', strictEvalContext) !== 11 ||
    Object.hasOwn(strictEvalContext, "evalLocal")) {
  throw new Error("strict new-context eval leaked a var into its context object");
}
if (runInNewContext("1; {}", {}) !== 1 ||
    runInNewContext("1; if (true) { 7 }", {}) !== 7) {
  throw new Error("new-context statement completion did not preserve UpdateEmpty values");
}
let returnWasSyntaxError = false;
try {
  runInNewContext("return 1", {});
} catch (error) {
  returnWasSyntaxError = error?.name === "SyntaxError";
}
if (!returnWasSyntaxError) {
  throw new Error("top-level return in new-context eval must be a SyntaxError");
}
console.log("new-context completion passed");
