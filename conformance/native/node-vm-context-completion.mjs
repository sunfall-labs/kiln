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
console.log("new-context completion passed");
