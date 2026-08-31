// A `var` belongs to its function, not to the block it is written in.
//
// The slot always went to the function's scope; the *instructions* did not. `NewCell` and the
// `undefined` a `var` starts at were emitted wherever the block containing the declaration
// happened to be compiled, so a `var` written inside one arm of an `if` had them inside that arm
// -- and the other arm found a slot that had never been boxed. When something captured the
// binding, that was `local slot 460 does not hold a captured cell`: not a JavaScript error a
// program could catch, but the process stopping with an internal message.
//
// `import "react-dom/server"` is exactly this, and nothing else in it was required to reach it:
//
//     if (typeof performance === "object" && typeof performance.now === "function") {
//       var localPerformance = performance;
//       var getCurrentTime = function () { return localPerformance.now(); };
//     } else {
//       var localDate = Date;
//       getCurrentTime = function () { return localDate.now(); };
//     }
//
// The closure is what makes it fatal rather than merely wrong: `getCurrentTime` is read from
// nested functions, so it is boxed, and the else path stores into a box that does not exist.

// The reachable half: a captured `var` declared in one branch and assigned in another.
function branches(flag) {
  if (flag) { var declared = 1; } else { declared = 2; }
  const read = () => declared;
  return read();
}
console.log("branches", branches(false), branches(true));

function scheduler(flag) {
  if (flag) {
    var performanceLike = { now: () => 9 };
    var currentTime = function () { return performanceLike.now(); };
  } else {
    var dateLike = { now: () => 10 };
    currentTime = function () { return dateLike.now(); };
  }
  return currentTime();
}
console.log("scheduler", scheduler(false), scheduler(true));

// Every other statement that shares the function's variable environment.
function loops(n) {
  for (let i = 0; i < n; i++) { var fromFor = i; }
  for (var fromForIn in { a: 1 }) { var seen = fromForIn; }
  for (var fromForOf of []) { var never = fromForOf; }
  const read = () => [fromFor, seen, never].join(",");
  return read();
}
console.log("loops", loops(0), "|", loops(1));

function guarded(flag) {
  try { if (flag) throw 1; var fromTry = "try"; } catch (error) { fromTry = "catch"; }
  switch (flag ? 1 : 2) { case 1: var fromSwitch = "one"; break; default: fromSwitch = "two"; }
  while (flag) { var fromWhile = "while"; break; }
  const read = () => `${fromTry}/${fromSwitch}/${fromWhile}`;
  return read();
}
console.log("guarded", guarded(true), guarded(false));

// The quiet half: a `var` reads as `undefined` from the top of its *function*, whether or not
// the block it was written in ever ran.
function unentered(flag) {
  if (flag) { var never = 1; }
  const read = () => typeof never;
  return `${typeof never}/${read()}`;
}
console.log("unentered", unentered(false), unentered(true));

// A nested function owns its own `var`s, so the two do not share a binding.
function nested() {
  var shared = "outer";
  function inner() { if (false) { var shared = "inner"; } return typeof shared; }
  return `${shared}/${inner()}`;
}
console.log("nested", nested());

// A `let` stays block-scoped and keeps its dead zone, which is what the hoist must not touch.
function lexical(flag) {
  let answers = [];
  if (flag) { let scoped = "in"; answers.push(scoped); } else { answers.push("out"); }
  try { (() => scoped)(); answers.push("leaked"); } catch (error) { answers.push(error.constructor.name); }
  return answers.join(",");
}
console.log("lexical", lexical(true), lexical(false));
