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
