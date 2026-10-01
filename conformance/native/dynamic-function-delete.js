const remove = Function("object", "key", `
  var local = 1;
  var first = delete object.value;
  var second = delete object[key];
  var third = delete object.missing;
  var fourth = delete local;
  var fifth = delete (local);
  var sixth = delete ((local));
  return first + "," + second + "," + third + "," + fourth + "," +
    fifth + "," + sixth + "," + ("value" in object) + "," + (key in object);
`);
const object = { value: 1, computed: 2 };
if (remove(object, "computed") !== "true,true,true,false,false,false,false,false") {
  throw new Error("dynamic delete removes only the referenced own properties");
}
const parenthesized = { value: 1, nested: 2 };
if (!Function("object", "return delete (object.value);")(parenthesized) || "value" in parenthesized) {
  throw new Error("parentheses preserve a member delete reference");
}
if (!Function("object", "return delete ((object.nested));")(parenthesized) || "nested" in parenthesized) {
  throw new Error("nested parentheses preserve a member delete reference");
}
const parentBase = { nested: { value: 2 } };
if (!Function("object", "return delete (object.nested).value;")(parentBase) ||
    "value" in parentBase.nested || !parentBase.nested) {
  throw new Error("a parenthesized object base keeps the outer delete reference");
}
const keywords = Function("return ({ delete: 1, finally: 2 }).delete + ({ finally: 3 }).finally;")();
if (keywords !== 4) throw new Error("new statement tokens remain valid property names");
const keywordMethods = Function(
  '"use strict"; return ({ delete(value) { return value; }, finally() { return 4; } });'
)();
if (keywordMethods.delete(7) !== 7 || keywordMethods.finally() !== 4) {
  throw new Error("strict method names that are keywords remain valid");
}
let called = 0;
if (Function("fn", "return delete fn();")(() => { called++; return 1; }) !== true || called !== 1) {
  throw new Error("delete of a call result evaluates it without deleting its callee");
}
if (Function("fn", "return delete (fn());")(() => { called++; return 1; }) !== true || called !== 2) {
  throw new Error("parenthesized delete of a call evaluates the call exactly once");
}
const locked = {};
Object.defineProperty(locked, "value", { value: 1, configurable: false });
let strictDeleteThrew = false;
try {
  Function("object", '"use strict"; return delete ((object.value));')(locked);
} catch (error) {
  strictDeleteThrew = error instanceof TypeError;
}
if (!strictDeleteThrew) throw new Error("strict delete rejects a non-configurable property");
if (Function("object", "return delete ((object.value));")(locked) !== false) {
  throw new Error("nested parentheses preserve a failed member deletion result");
}
if (Function("value", "return delete (value);")(1) !== false ||
    Function("value", "return delete ((value));")(1) !== false) {
  throw new Error("parenthesized identifier deletion returns false for local bindings");
}

let strictIdentifierDeleteRejectedAtConstruction = false;
try {
  Function('"use strict"; if (false) delete neverDeclared;');
} catch (error) {
  strictIdentifierDeleteRejectedAtConstruction = error instanceof SyntaxError;
}
if (!strictIdentifierDeleteRejectedAtConstruction) {
  throw new Error("strict delete of an identifier is rejected while constructing the function");
}

console.log("dynamic delete:", object.value, object.computed);
