function units(value) {
  if (typeof value !== 'string') return typeof value + ':' + String(value);
  const out = [];
  for (let i = 0; i < value.length; i++) out.push(value.charCodeAt(i));
  return out.join(',');
}
function show(label, thunk) {
  try {
    const value = thunk();
    if (Array.isArray(value)) console.log(label, value.map(units).join('|'));
    else console.log(label, units(value));
  } catch (error) {
    console.log(label, 'THROW', error.name, String(error.message).slice(0, 80));
  }
}
const high = String.fromCharCode(0xd800);
const low = String.fromCharCode(0xdc00);
const replacement = String.fromCharCode(0xfffd);
function tag(strings) { return [strings.raw[0], strings[0]]; }
show('eval-raw-string-high', () => eval('"' + high + '"'));
show('eval-raw-string-replacement', () => eval('"' + replacement + '"'));
show('eval-escaped-string-high', () => eval('"' + '\\uD800' + '"'));
show('eval-raw-template', () => eval('tag`' + high + '`'));
show('eval-raw-regexp', () => eval('/' + high + '/').source);
show('eval-raw-comment', () => eval('// ' + high + '\n42'));
show('eval-invalid-outside-literal', () => eval(high));
show('function-raw-string-high', () => new Function('return "' + high + '"')());
show('function-escaped-string-high', () => new Function('return "' + '\\uD800' + '"')());
show('function-source-raw-string-high', () => new Function('return "' + high + '"').toString());
show('raw-low', () => eval('"' + low + '"'));
if (typeof ShadowRealm === 'function') {
  show('shadow-eval-raw-string-high', () => new ShadowRealm().evaluate('"' + high + '"'));
  show('shadow-eval-raw-template', () => new ShadowRealm().evaluate('tag`' + high + '`'));
} else {
  console.log('shadow-eval-raw-string-high', 'UNAVAILABLE');
  console.log('shadow-eval-raw-template', 'UNAVAILABLE');
}
