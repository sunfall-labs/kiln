import { runInNewContext } from "node:vm";

class MutableClassDeclaration {}
MutableClassDeclaration = 1;
if (MutableClassDeclaration !== 1) {
  throw new Error("class declaration binding became immutable");
}

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
const dynamicClass = runInNewContext("new (class A { a = 1 })()", {});
if (dynamicClass.a !== 1 || !Object.hasOwn(dynamicClass, "a") ||
    dynamicClass.constructor.name !== "A") {
  throw new Error("new-context class expression did not initialize a public field");
}
const namedClassConstructor = runInNewContext("(class Named { self = Named })", {});
const classConstructor = namedClassConstructor.prototype.constructor;
const prototypeWriteAccepted = Reflect.set(namedClassConstructor, "prototype", {});
const constructorIsEnumerable = Object.keys(namedClassConstructor.prototype).includes("constructor");
namedClassConstructor.prototype.constructor = 42;
const constructorIsWritable = namedClassConstructor.prototype.constructor === 42;
const constructorIsConfigurable = delete namedClassConstructor.prototype.constructor;
if (new namedClassConstructor().self !== namedClassConstructor ||
    prototypeWriteAccepted || constructorIsEnumerable || !constructorIsWritable ||
    !constructorIsConfigurable || typeof classConstructor !== "function") {
  throw new Error("new-context class identity or constructor descriptors were incorrect");
}
let inheritedFieldSetterCalls = 0;
const fieldContext = { inheritedFieldSetterCalls };
const fieldResult = runInNewContext(
  "Object.defineProperty(Object.prototype, 'field', { set() { inheritedFieldSetterCalls++; } }); " +
  "new (class { field = 3 })()",
  fieldContext,
);
if (fieldResult.field !== 3 || !Object.hasOwn(fieldResult, "field") ||
    fieldContext.inheritedFieldSetterCalls !== 0) {
  throw new Error("new-context class field invoked an inherited setter");
}
const classFieldContext = {};
const classFieldResults = runInNewContext(
  "let count = 0; const C = class A { first = ++count; second = this.first + 1 }; " +
    "[count, new C().first, new C().second, count]",
  classFieldContext,
);
if (classFieldResults.join(",") !== "0,1,3,2") {
  throw new Error(`new-context class fields lost lexical/order semantics: ${classFieldResults}`);
}
const classCallCaughtInContext = runInNewContext(
  "let caught = false; try { (class A {})() } catch (error) { caught = error?.name === 'TypeError' }; caught",
  {},
);
if (classCallCaughtInContext !== true) {
  throw new Error("new-context class call did not expose a catchable TypeError");
}
const nestedClassCallCaught = runInNewContext(
  "let caught = false; try { new (class Outer { field = (class Inner {})() })() } " +
    "catch (error) { caught = error?.name === 'TypeError' }; caught",
  {},
);
if (nestedClassCallCaught !== true) {
  throw new Error("class field initializer bypassed class-call protection");
}
const strictClassInitializerCaught = runInNewContext(
  "let caught = false; try { new (class Named { field = (Named = null) })() } " +
    "catch (error) { caught = error?.name === 'TypeError' }; caught",
  {},
);
if (strictClassInitializerCaught !== true) {
  throw new Error("named class field initializer did not use strict immutable class scope");
}
const dynamicClassContext = {};
runInNewContext("C = class A {}", dynamicClassContext);
let classCallCaughtByCaller = false;
try {
  dynamicClassContext.C();
} catch (error) {
  classCallCaughtByCaller = error?.name === "TypeError";
}
if (!classCallCaughtByCaller) {
  throw new Error("dynamic class call did not throw a TypeError to its caller");
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
