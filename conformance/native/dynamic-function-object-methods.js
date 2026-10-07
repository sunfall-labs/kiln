const make = new Function(
  "return { read(value) { return this.prefix + value; }, " +
    "nested: { double(value) { return value * 2; } } };",
);
const methods = make();
methods.prefix = "value:";
console.log("method receiver:", methods.read(7));
console.log("nested method:", methods.nested.double(21));

const captured = new Function(
  "const prefix = 'captured:'; return { read(value) { return prefix + value; } };",
)();
console.log("method capture:", captured.read(9));
const generatedArrows = Function("return { block: value => { return value + 2; }, " +
  "expression: value => value + 3 };")();
console.log("single-parameter arrows:", generatedArrows.block(4), generatedArrows.expression(5));
console.log("method name:", methods.read.name);
console.log("method rejects construction:", (() => {
  try { new methods.read(); return false; } catch (error) { return error.name === "TypeError"; }
})());
const reservedMethod = new Function("return { 'while'() { return 53; } };")();
console.log("quoted control-word method:", reservedMethod.while());

const quotedOptions = Function('return {"fn": function(value) { return value + 1; }, "inverse": function() { return "empty"; }, "é": "unicode", "": 7};')();
if (quotedOptions.fn(8) !== 9 || quotedOptions.inverse() !== 'empty' || quotedOptions['é'] !== 'unicode' || quotedOptions[''] !== 7) {
  throw new Error('dynamic quoted property names must survive value parsing');
}
console.log('quoted callback options:', quotedOptions.fn(8), quotedOptions.inverse(), quotedOptions['é'], quotedOptions['']);
