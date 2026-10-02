// kiln-native: profile=native-release
// A typed recursive self-call can leave its typed entry when a relied-on built-in changes.
// The generic fallback must receive the running closure's upvalue table, or NULL when the
// closure is captureless.
const originalRound = Math.round;
const patchedRound = value => value + 100;

function captureless(n) {
  if (n === 0) return 0;
  if (n === 2) Math.round = patchedRound;
  return Math.round(n) + captureless(n - 1);
}
console.log("captureless", captureless(3));
Math.round = originalRound;

function makeCaptured(offset) {
  function recurse(n) {
    if (n === 0) return offset;
    if (n === 2) Math.round = patchedRound;
    return Math.round(n) + recurse(n - 1);
  }
  return recurse;
}
const captured = makeCaptured(5);
console.log("captured", captured(3));
Math.round = originalRound;
