// Direct span materialisation must preserve units at template/prefix/suffix boundaries,
// while replacement protocol dispatch, callbacks and coercion retain their side effects.
const high = '\ud83d', low = '\ude00';
const units = value => Array.from({length:value.length}, (_,i) => value.charCodeAt(i));
const check = (name, value) => console.log(name, JSON.stringify(units(value)));
check('prefix-pair', (high+'x').replace(/x/, low));
check('suffix-pair', ('x'+low).replace('x', high));
check('template-pair', 'x'.replace(/x/, high+low));
check('capture-pair', (high+'x'+low).replace(/(x)/, '$`$1$\''));
check('unmatched-capture', 'ab'.replace(/(a)|(z)/g, '$2$1$12$99$$'));
check('named-capture', (high+'x').replace(/(?<h>.)x/, '$<h>'+low));
check('unknown-named', 'ab'.replace(/(?<a>a)/, '$<missing>$<a>'));
check('unrecognised-template', 'a'.replace(/a/, '$0$99$<missing>$'));
check('empty-global', (high+low).replace(/(?:)/g, '-'));
check('empty-unicode-global', (high+low).replace(/(?:)/gu, '-'));
check('literal-empty', (high+low).replaceAll('', '-'));
check('latin1', '\u00ffx\u0000'.replace(/x/, '\u00e9'));
check('zero-width', 'abc'.replace(/^/g, ''));
check('no-match-regexp', (high+'ab'+low).replace(/missing/g, 'x'));
check('no-match-literal', (high+'ab'+low).replaceAll('missing', 'x'));
const regexp = /x/g;
regexp.lastIndex = 9;
console.log('no-match-cursor', 'abc'.replace(regexp, '$&'), regexp.lastIndex);
let conversions = 0;
const replacement = {toString(){ conversions++; return high; }};
console.log('coercion-no-match', 'abc'.replace(/x/g, replacement), conversions);
console.log('coercion-literal-no-match', 'abc'.replaceAll('x', replacement), conversions);
check('coercion-match', 'x'.replace(/x/, replacement));
check('coercion-literal-match', 'x'.replace('x', replacement));
console.log('conversion-count', conversions);
console.log('missing-replacement', 'x'.replace('x'), 'x'.replace(/x/));
try { 'abc'.replace(/missing/, Symbol('x')); }
catch (error) { console.log('symbol-regexp-replacement', error.name); }
try { 'abc'.replace('missing', Symbol('x')); }
catch (error) { console.log('symbol-literal-replacement', error.name); }
try { 'abc'.replace(/missing/, {toString(){ throw new Error('conversion-error'); }}); }
catch (error) { console.log('conversion-throw', error.message); }
const calls = [];
check('callbacks', 'xax'.replace(/(x)/g, (match, capture, index, subject) => {
  calls.push([match, capture, index, subject]);
  const scratch = Array.from({length:16}, (_,i) => ({i}));
  return high+String(scratch[0].i)+low;
}));
console.log('callback-arguments', JSON.stringify(calls));
const protocol = { [Symbol.replace](subject, replacement) {
  console.log('custom-protocol', subject, replacement);
  return 'custom';
}};
console.log('custom-result', 'input'.replace(protocol, 'output'));
try { 'x'.replace(/x/g, () => { throw new Error('callback-error'); }); }
catch (error) { console.log('callback-throw', error.message); }
let large = '';
for (let i=0; i<128; i++) large += 'a'+high+'x'+low;
const rendered = large.replace(/x/g, '$&'+low+'literal'+high);
let checksum = 0;
for (let i=0; i<rendered.length; i++) checksum = (checksum + rendered.charCodeAt(i)) >>> 0;
console.log('large', rendered.length, checksum);
