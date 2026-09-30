function join(a, b) { return a + ':' + b; }
function factory(source) {
  let captured = 'scope';
  return join('prefix', eval(source)());
}
const source = '(function () { return captured; })';
console.log(factory(source));
