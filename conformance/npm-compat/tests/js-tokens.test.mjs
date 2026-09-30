// kiln eval smoke test (authored for kiln, not from porffor).
import jsTokens from 'js-tokens';

const tokens = [...jsTokens('var x = 42; // comment')].filter((t) => t.type !== 'WhiteSpace');
console.log(JSON.stringify(tokens.map((t) => t.type + ':' + t.value)));
const jsx = [...jsTokens('<div>hi</div>', { jsx: true })].filter((t) => t.type !== 'WhiteSpace');
console.log(JSON.stringify(jsx.map((t) => t.type + ':' + t.value)));
const strs = [...jsTokens('`a${1}b` "s" \'t\'')].map((t) => t.type);
console.log(JSON.stringify(strs));
