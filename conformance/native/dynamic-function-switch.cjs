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
const switchShadowsOuterVar = Function(
  "var x = 1; switch (0) { case 0: let x = 2; return x; }",
);
if (switchShadowsOuterVar() !== 2) {
  throw new Error("switch lexical declarations may shadow an outer function var");
}
const switchIgnoresNestedFunctionVar = Function(
  "switch (0) { case 0: let x = 2; function nested() { var x = 3; } return x; }",
);
if (switchIgnoresNestedFunctionVar() !== 2) {
  throw new Error("nested function vars do not conflict with the switch CaseBlock");
}
const switchFunctionInstantiation = Function(
  "switch (0) { case 0: let outer = 2; function nested() { return outer; } return nested(); }",
);
if (switchFunctionInstantiation() !== 2) {
  throw new Error("switch function declarations are initialized in the CaseBlock scope");
}
const unselectedSwitchFunction = Function(
  "switch (1) { case 0: function hoisted() {} case 1: return typeof hoisted; }",
);
if (unselectedSwitchFunction() !== "function") {
  throw new Error("switch function declarations initialize before clause selection");
}
const strictSwitchFunction = Function(
  '"use strict"; switch (0) { case 0: function scoped() {} } return typeof scoped;',
);
if (strictSwitchFunction() !== "undefined") {
  throw new Error("switch function declarations stay inside the CaseBlock environment");
}
const annexBFunctionBeforeStatement = Function(
  "switch (0) { case 0: return typeof annexed; function annexed() {} }",
);
if (annexBFunctionBeforeStatement() !== "function") {
  throw new Error("selected sloppy switch functions alias the var environment on clause entry");
}
const annexBUnselectedFunction = Function(
  "switch (1) { case 0: function annexed() {} } return typeof annexed;",
);
if (annexBUnselectedFunction() !== "undefined") {
  throw new Error("unselected sloppy switch functions do not initialize their var alias");
}
const annexBVarFunction = Function(
  "var annexed; switch (0) { case 0: function annexed() { return 7; } } return annexed();",
);
if (annexBVarFunction() !== 7) {
  throw new Error("sloppy switch function declarations update an existing var binding");
}
const annexBParameter = Function(
  "annexed", "switch (0) { case 0: function annexed() { return 7; } } return annexed;",
);
if (annexBParameter(5) !== 5) {
  throw new Error("sloppy switch function declarations do not overwrite a parameter alias");
}
const unselectedAnnexBParameter = Function(
  "annexed", "switch (1) { case 0: function annexed() { return 7; } } return annexed;",
);
if (unselectedAnnexBParameter(5) !== 5) {
  throw new Error("unselected sloppy switch functions do not overwrite a parameter alias");
}
const annexBLexical = Function(
  "let annexed = 5; switch (0) { case 0: function annexed() { return 7; } } return annexed;",
);
if (annexBLexical() !== 5) {
  throw new Error("sloppy switch functions do not replace an outer lexical binding");
}
const unselectedAnnexBLexical = Function(
  "let annexed = 5; switch (1) { case 0: function annexed() { return 7; } } return annexed;",
);
if (unselectedAnnexBLexical() !== 5) {
  throw new Error("unselected sloppy switch functions do not replace an outer lexical binding");
}
const asiSwitchFunction = Function(
  "switch (0) { case 0: let captured = 2\n function afterAsi() { return captured; } return afterAsi(); }",
);
if (asiSwitchFunction() !== 2) {
  throw new Error("ASI-separated switch function declarations capture the CaseBlock scope");
}
globalThis.__switchFunctionConflictCount = 0;
let functionConflictThrew = false;
try {
  Function("switch ((globalThis.__switchFunctionConflictCount++, 0)) { " +
    "case 0: function duplicate() {} default: let duplicate; }")();
} catch (error) { functionConflictThrew = error.name === "SyntaxError"; }
if (!functionConflictThrew || globalThis.__switchFunctionConflictCount !== 0) {
  throw new Error("switch function declarations conflict with lexical names before evaluation");
}
delete globalThis.__switchFunctionConflictCount;
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

globalThis.__switchMixedDeclarationCount = 0;
let mixedDeclarationThrew = false;
try {
  Function("switch ((globalThis.__switchMixedDeclarationCount++, 0)) { " +
    "case 0: let mixed; { var mixed; } }")();
} catch (error) { mixedDeclarationThrew = error.name === "SyntaxError"; }
if (!mixedDeclarationThrew || globalThis.__switchMixedDeclarationCount !== 0) {
  throw new Error("switch lexical declarations conflict with function-scoped var before evaluation");
}
delete globalThis.__switchMixedDeclarationCount;

const asiDeclaredCaseBinding = Function(
  "second", "switch (2) { case 0: let first = 1\n let second = 2; case second: break; } return 7;",
);
tdzThrew = false;
try { asiDeclaredCaseBinding(9); } catch (error) { tdzThrew = error.name === "ReferenceError"; }
if (!tdzThrew) throw new Error("ASI-separated switch declarations are predeclared in the case scope");

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
