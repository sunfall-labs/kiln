const log = [];
const returned = Function("log", "try { return 3; } finally { log.push('finally'); }");
if (returned(log) !== 3 || log.join(",") !== "finally") {
  throw new Error("finally runs while preserving a prior return");
}

const overridden = Function("try { return 3; } finally { return 4; }");
if (overridden() !== 4) throw new Error("finally return replaces prior return");

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

console.log("dynamic try/finally:", log.join(","));
