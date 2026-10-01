const log = [];
const returned = Function("log", "try { return 3; } finally { log.push('finally'); }");
if (returned(log) !== 3 || log.join(",") !== "finally") {
  throw new Error("finally runs while preserving a prior return");
}

const overridden = Function("try { return 3; } finally { return 4; }");
if (overridden() !== 4) throw new Error("finally return replaces prior return");

const overriddenThrow = Function("try { return 3; } finally { throw 5; }");
let overrideValue;
try {
  overriddenThrow();
} catch (error) {
  overrideValue = error;
}
if (overrideValue !== 5) throw new Error("finally throw replaces a pending return");

const thrown = Function("log", "try { throw 7; } finally { log.push('throw-finally'); }");
let caught;
try {
  thrown(log);
} catch (error) {
  caught = error;
}
if (caught !== 7 || log.join(",") !== "finally,throw-finally") {
  throw new Error("finally preserves a prior throw");
}

const caughtAndFinalized = Function("log", `
  try { throw 8; }
  catch (error) { log.push(error); return 9; }
  finally { log.push("catch-finally"); }
`);
if (caughtAndFinalized(log) !== 9 || log.join(",") !== "finally,throw-finally,8,catch-finally") {
  throw new Error("finally runs after catch and preserves its return");
}

const returnedObject = {};
const preserveObject = Function("value", `
  try { return value; }
  finally { for (var index = 0; index < 40; index++) Object.create(null); }
`);
if (preserveObject(returnedObject) !== returnedObject) {
  throw new Error("normal finally preserves the pending return value across allocations");
}

const flowEvents = [];
const labeledFlow = Function("events", `
  var index = 0;
  outer: for (; index < 3; index++) {
    try {
      if (index === 0) continue outer;
      break outer;
    } finally {
      events.push("finally-" + index);
    }
  }
  return index;
`);
if (labeledFlow(flowEvents) !== 1 || flowEvents.join(",") !== "finally-0,finally-1") {
  throw new Error("finally preserves and propagates labeled continue/break completions");
}

const nestedFinallyFlow = Function(`
  var reached = 0;
  outer: {
    try { break outer; }
    finally { while (true) { reached++; break; } }
  }
  return reached;
`);
if (nestedFinallyFlow() !== 1) {
  throw new Error("a nested loop break in normal finally preserves the pending labeled break");
}

const nestedFinallyContinue = Function(`
  var count = 0;
  outer: for (; count < 2; count++) {
    try { continue outer; }
    finally { while (true) { break; } }
  }
  return count;
`);
if (nestedFinallyContinue() !== 2) {
  throw new Error("a nested loop break in normal finally preserves the pending continue");
}

console.log("dynamic try/finally:", log.join(","));
