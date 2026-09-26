// A function made by `new Function` that returns a closure: the closure reads the outer body's
// bindings through a captured environment object. That object is an ordinary object, so it is
// shaped whenever its first binding's name can branch from the root shape -- which, since the
// root stopped filling up during the bootstrap, is almost always. Ajv's compiled validators are
// exactly this form (`const func0 = scope.func[0]; return function validate(data) {...}`).
const s = { func: [(x) => x + 1], schema: [{ type: "integer" }] };
console.log("typeof", new Function("s", "const a = s.func[0]; return typeof a;")(s));
console.log("const", new Function("s", "const a = s.func[0]; return function (d) { return a(d); };")(s)(41));
console.log("let", new Function("let a = 5; return function () { return a; };")()());
console.log("var", new Function("var a = 6; return function () { return a; };")()());
const make = new Function("scope",
  'const schema0 = scope.schema[0]; const f = scope.func[0];' +
  'return function validate(data, {instancePath = "", rootData = data} = {}) {' +
  '  const err = {instancePath, keyword: "type", params: {type: schema0.type}};' +
  '  return [f(data), err.params.type, instancePath, rootData === data];' +
  '};');
const validate = make(s);
console.log("validator", JSON.stringify(validate(1)), JSON.stringify(validate(2, { instancePath: "/x" })));
