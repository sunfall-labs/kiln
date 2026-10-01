const JSON5 = require('json5');

console.log('--- Numbers & Special Values ---');
const numbersInput = `{
  // numeric representations
  hex: 0x2A,
  hexUpper: 0XFF,
  leadingDot: .75,
  trailingDot: 100.,
  explicitPlus: +50,
  exponential: 1e4,
  nan: NaN,
  posInf: Infinity,
  negInf: -Infinity
}`;
const parsedNumbers = JSON5.parse(numbersInput);
console.log('hex:', parsedNumbers.hex);
console.log('leadingDot:', parsedNumbers.leadingDot);
console.log('trailingDot:', parsedNumbers.trailingDot);
console.log('explicitPlus:', parsedNumbers.explicitPlus);
console.log('nan isNaN:', Number.isNaN(parsedNumbers.nan));
console.log('posInf:', parsedNumbers.posInf);
console.log('negInf:', parsedNumbers.negInf);

console.log('--- Strings & Comments & Trailing commas ---');
const docInput = `/* Multi-line
   configuration */
{
  unquotedKey: 'single-quoted string',
  'quoted-key': 'value',
  multilineString: 'first line \\
second line',
  list: [
    1,
    2, // inline comment
    3,
  ],
}`;
const parsedDoc = JSON5.parse(docInput);
console.log('unquotedKey:', parsedDoc.unquotedKey);
console.log('multilineString:', parsedDoc.multilineString);
console.log('list:', JSON.stringify(parsedDoc.list));

console.log('--- Reviver & Replacer ---');
const withReviver = JSON5.parse('{ date: "2026-01-01", count: "42" }', (key, val) => {
  if (key === 'count') return Number(val);
  return val;
});
console.log('reviver count type:', typeof withReviver.count, withReviver.count);

console.log('--- Stringify with options ---');
const stringified = JSON5.stringify({
  greeting: 'hello',
  items: [1, 2],
  active: true
}, null, 2);
console.log('stringified lines count:', stringified.split('\n').length);
console.log('contains unquoted key:', stringified.includes('greeting:'));

console.log('--- Syntax error detection ---');
try {
  JSON5.parse('{ unclosed: "string }');
} catch (e) {
  console.log('caught syntax error at lineNumber:', typeof e.lineNumber === 'number' || typeof e.message === 'string');
}
