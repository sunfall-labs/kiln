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
console.log("compound", new Function("value", "value += 1; return value;")(2));
console.log(
  "loop compound",
  new Function("return function () { let sum = 0; for (let i = 0; i < 3; i++) sum += i; return sum; };")()(),
);
const make = new Function("scope",
  'const schema0 = scope.schema[0]; const f = scope.func[0];' +
  'return function validate(data, {instancePath = "", rootData = data} = {}) {' +
  '  const err = {instancePath, keyword: "type", params: {type: schema0.type}};' +
  '  return [f(data), err.params.type, instancePath, rootData === data];' +
  '};');
const validate = make(s);
console.log("validator", JSON.stringify(validate(1)), JSON.stringify(validate(2, { instancePath: "/x" })));
const compiled = new Function(
  'function root() { return "rendered"; } return {root: root};',
)();
console.log("function declaration", typeof compiled.root, compiled.root());
const rendered = new Function(
  'function root() { try { throw "caught"; } catch (error) { return {value: error}; } } ' +
  'return {root: root};',
)();
console.log("dynamic try catch", rendered.root().value);

// Template packages such as Underscore, Eta, and EJS build functions with `new Function`,
// multiple declarators, and a `with` environment whose names fall back behind function locals.
const renderTemplate = new Function(
  "locals",
  "var __t, __p = '', print = function(value) { __p += value; }; " +
    "with (locals || {}) { __p += 'Hello ' + name; print('!'); } return __p;",
);
console.log("dynamic with", renderTemplate({ name: "Ada" }));
const dynamicArrow = new Function(
  "items",
  "let total = 0; items.forEach(i => { total += i; }); return total;",
);
console.log("dynamic arrow", dynamicArrow([1, 2, 3]));
console.log(
  "dynamic arrow expression",
  JSON.stringify(new Function("return [1, 2, 3].map(x => x * 2);")()),
);
