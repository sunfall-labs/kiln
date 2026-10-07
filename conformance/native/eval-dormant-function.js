// A direct eval in an uncalled function is not a runtime compiler requirement for this
// executable. Native emission should discard the unreachable function before applying the
// reachable DirectEval capability check.
function neverCalled() {
  return eval("1 + 1");
}

console.log("dormant eval was pruned");
