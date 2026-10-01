const run = Function("value", "events", `
  var result = 0;
  switch (value) {
    case (events.push("first"), 1): result += 10; break;
    default: result += 100;
    case (events.push("last"), 2): result += 20; break;
    case 3: result += 30;
  }
  return result;
`);
const events = [];
if (run(2, events) !== 20 || events.join(",") !== "first,last") {
  throw new Error("switch evaluates case expressions in order and matches after default");
}
if (run(9, []) !== 120) throw new Error("switch enters default and falls through");
if (run(3, []) !== 30) throw new Error("switch case matching is strict and fallthrough stops at break");

const labeled = Function(`
  var count = 0;
  outer: while (count < 3) {
    switch (count++) {
      case 0: continue outer;
      case 1: break;
      default: break outer;
    }
    count += 10;
  }
  return count;
`);
if (labeled() !== 12) throw new Error("switch propagates continue to outer loop and consumes switch break");
const labeledBreak = Function("outer: while (true) { switch (0) { default: break outer; } } return 3;");
if (labeledBreak() !== 3) throw new Error("switch propagates labeled break to outer label");
const switchLabel = Function(`
  var result = 0;
  done: switch (1) {
    case 1:
      while (true) { result = 1; break done; }
      result = 99;
  }
  return result;
`);
if (switchLabel() !== 1) throw new Error("an inner loop must not consume the switch label");

const nested = Function(`
  var value = 0;
  switch (1) {
    case 1:
      switch (2) { case 2: value = 7; break; default: value = 99; }
      value += 1;
      break;
    default: value = 99;
  }
  return value;
`);
if (nested() !== 8) throw new Error("nested switch consumes only its own break");

const varCapture = Function(`
  function read() { return inside; }
  switch (1) { case 1: var inside = 17; }
  return read();
`);
if (varCapture() !== 17) throw new Error("switch var declaration targets function scope");

const switchDiscriminantScope = Function(
  "x", "switch (x) { case 0: let x = 23; return x; }",
);
if (switchDiscriminantScope(0) !== 23) {
  throw new Error("switch discriminant resolves before entering the case lexical scope");
}
const switchCaseTdz = Function(
  "outer", "switch (0) { case outer: let outer = 23; } return outer;",
);
let tdzThrew = false;
try { switchCaseTdz(0); } catch (error) { tdzThrew = error.name === "ReferenceError"; }
if (!tdzThrew) throw new Error("switch case expression sees the lexical binding in its TDZ");

const laterClauseTdz = Function(
  "outer", "switch (0) { case 1: break; case outer: break; default: let outer = 29; } return outer;",
);
tdzThrew = false;
try { laterClauseTdz(0); } catch (error) { tdzThrew = error.name === "ReferenceError"; }
if (!tdzThrew) throw new Error("unselected switch clauses still instantiate lexical bindings");

const switchVarStillFunctionScoped = Function(
  "switch (0) { case 0: var switchVar = 31; } return switchVar;",
);
if (switchVarStillFunctionScoped() !== 31) throw new Error("switch var remains function scoped");

globalThis.__switchDuplicateInitializerCount = 0;
let duplicateThrew = false;
try {
  Function("switch ((globalThis.__switchDuplicateInitializerCount++, 0)) { " +
    "case 0: let repeated; default: const repeated = 1; }")();
} catch (error) { duplicateThrew = error.name === "SyntaxError"; }
if (!duplicateThrew || globalThis.__switchDuplicateInitializerCount !== 0) {
  throw new Error("duplicate switch lexical names are rejected before runtime evaluation");
}
delete globalThis.__switchDuplicateInitializerCount;

const skippedInitializer = Function(
  "switch (0) { case 0: break; default: let later = (globalThis.__switchInitializerRan = true); }",
);
globalThis.__switchInitializerRan = false;
skippedInitializer();
if (globalThis.__switchInitializerRan) throw new Error("unselected lexical initializer must not run");
delete globalThis.__switchInitializerRan;

console.log("switch evaluation:", events.join(","), run(9, []), run(3, []), labeled(), labeledBreak(), nested(), switchVarStillFunctionScoped());
