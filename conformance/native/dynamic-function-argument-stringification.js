const order = [];
const firstParameter = { toString() { order.push('first'); return 'left'; } };
const secondParameter = { toString() { order.push('second'); return 'right'; } };
const body = { toString() { order.push('body'); return 'return left + right;'; } };
const generated = Function(firstParameter, secondParameter, body);
const constructed = new Function("domprops", "exports", "return domprops.kind + exports.value;");

let parameterSymbolThrows = false;
try {
  Function(Symbol('parameter'), 'return 1;');
} catch (error) {
  parameterSymbolThrows = error instanceof TypeError;
}

let bodySymbolThrows = false;
try {
  Function('value', Symbol('body'));
} catch (error) {
  bodySymbolThrows = error instanceof TypeError;
}

console.log(
  generated(19, 23),
  order.join(','),
  parameterSymbolThrows,
  bodySymbolThrows,
  typeof constructed,
  constructed({ kind: "value=" }, { value: 42 }),
);
