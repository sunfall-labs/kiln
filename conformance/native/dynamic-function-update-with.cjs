// This regression needs sloppy direct eval: `.js` probes run as ESM and inherit strict mode,
// where a `with` statement in the evaluated function expression is a SyntaxError.
function updateWithLexicalShadow() {
  const value = 100;
  const object = { value: 5 };
  const result = eval("(function() { with (object) { return value++; } })()");
  return [result, object.value, value];
}

const result = updateWithLexicalShadow();
if (result[0] !== 5 || result[1] !== 6 || result[2] !== 100) {
  throw new Error("direct eval update must resolve through the sloppy with environment");
}
console.log(result.join(","));
