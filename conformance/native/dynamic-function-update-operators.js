// Prefix and postfix updates must accept identifier references as well as members in dynamic
// Function bodies. The result and final binding distinguish their value/order semantics.
const update = new Function("value", "return [++value, value++, --value, value--, value];");
console.log(update(2).join(","));
console.log(update("4").join(","));
console.log(update(2n).map(String).join(","));

let coercions = 0;
const converted = update({
  valueOf() { coercions++; return 6; },
});
console.log(converted.join(","), coercions);

const member = new Function("object", "return [++object.value, object.value++, object.value];");
const object = { value: 4 };
console.log(member(object).join(","));

const outer = { value: 100 };
const inner = {};
Object.defineProperty(inner, "value", {
  configurable: true,
  get() {
    delete inner.value;
    outer.value = 200;
    return { valueOf() { return 5; } };
  },
});
const updateWith = new Function("outer", "inner",
  "with (outer) { with (inner) { return value++; } }");
console.log(updateWith(outer, inner), inner.value, outer.value);

let missingThrows = false;
try { new Function("return ++dynamicUpdateMissing;")(); }
catch (error) { missingThrows = error instanceof ReferenceError; }
console.log("unresolvable reference throws:", missingThrows);
let postfixMissingThrows = false;
try { new Function("return dynamicUpdateMissing++;")(); }
catch (error) { postfixMissingThrows = error instanceof ReferenceError; }
console.log("unresolvable postfix throws:", postfixMissingThrows);
let symbolThrows = false;
try { update(Symbol("numeric")); }
catch (error) { symbolThrows = error instanceof TypeError; }
console.log("symbol update throws:", symbolThrows);

function updateConst() {
  const fixed = 3;
  try { eval("++fixed"); }
  catch (error) { return error instanceof TypeError; }
  return false;
}
console.log("const update throws:", updateConst());

const namedUpdate = function namedUpdate() { return eval("++namedUpdate"); };
const namedResult = namedUpdate();
console.log("sloppy named update:", Number.isNaN(namedResult), namedUpdate.name);
