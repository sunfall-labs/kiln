// ES2026 JSON.parse reviver context.source preserves primitive lexical spelling.
const seen = [];
const parsed = JSON.parse('{"n":1e+03,"s":"\\u0061","a":[-0,true,null],"o":{"x":2}}',
  function (key, value, context) {
    seen.push([key, Object.prototype.hasOwnProperty.call(context, 'source') ? context.source : '<absent>']);
    return value;
  });
console.log(JSON.stringify(seen));

// A value changed by an earlier callback no longer carries source text; duplicate names use
// the final token, and the root primitive has an empty-string holder key.
const changed = JSON.parse('{"a":1,"b":2,"b":3}', function (key, value, context) {
  if (key === 'a') this.b = 9;
  return [key, value, context.source];
});
console.log(JSON.stringify(changed));
console.log(JSON.stringify(JSON.parse('-0', function (key, value, context) {
  return [key, Object.is(value, -0), context.source];
})));
