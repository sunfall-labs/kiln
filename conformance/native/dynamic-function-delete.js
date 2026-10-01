const remove = Function("object", "key", `
  var local = 1;
  var first = delete object.value;
  var second = delete object[key];
  var third = delete object.missing;
  var fourth = delete local;
  return first + "," + second + "," + third + "," + fourth + "," +
    ("value" in object) + "," + (key in object);
`);
const object = { value: 1, computed: 2 };
if (remove(object, "computed") !== "true,true,true,false,false,false") {
  throw new Error("dynamic delete removes only the referenced own properties");
}
const keywords = Function("return ({ delete: 1, finally: 2 }).delete + ({ finally: 3 }).finally;")();
if (keywords !== 4) throw new Error("new statement tokens remain valid property names");
if (Function("fn", "return delete fn();")(() => 1) !== true) {
  throw new Error("delete of a call result evaluates it without deleting its callee");
}
const locked = {};
Object.defineProperty(locked, "value", { value: 1, configurable: false });
let strictDeleteThrew = false;
try {
  Function("object", '"use strict"; return delete object.value;')(locked);
} catch (error) {
  strictDeleteThrew = error instanceof TypeError;
}
if (!strictDeleteThrew) throw new Error("strict delete rejects a non-configurable property");

console.log("dynamic delete:", object.value, object.computed);
