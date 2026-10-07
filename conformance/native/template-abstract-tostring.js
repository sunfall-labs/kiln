const symbol = Symbol('template');
let directSymbolThrew = false;
try {
  `${symbol}`;
} catch (error) {
  directSymbolThrew = error instanceof TypeError;
}
if (!directSymbolThrew) throw new Error('template interpolation must reject Symbol');
if (String(symbol) !== 'Symbol(template)') {
  throw new Error('the public String function must continue to accept Symbol');
}

const calls = [];
const value = {
  [Symbol.toPrimitive](hint) {
    calls.push(hint);
    return '\uD800';
  },
};
const text = `${value}`;
if (calls.join(',') !== 'string') throw new Error('template uses string-hint ToPrimitive');
if (text.length !== 1 || text.charCodeAt(0) !== 0xd800) {
  throw new Error('template ToString must preserve UTF-16 code units');
}

let shadowedString = false;
{
  const String = () => {
    shadowedString = true;
    return 'wrong';
  };
  if (`${42}` !== '42') throw new Error('template interpolation is an abstract operation');
}
if (shadowedString) throw new Error('template interpolation must not call global String');

console.log('template abstract ToString');
