// A live direct eval needs the source compiler at runtime. The native backend has no such
// compiler service, so this remains an explicit emission refusal rather than a partial answer.
function evaluate(source) {
  return eval(source);
}

console.log(evaluate("1 + 1"));
