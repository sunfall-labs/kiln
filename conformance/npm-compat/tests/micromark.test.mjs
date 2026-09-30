// kiln eval smoke test (authored for kiln, not from porffor).
import { micromark } from 'micromark';

console.log('heading:', micromark('# Hello *world*'));
console.log('link:', micromark('[kiln](https://example.com "title")'));
console.log('code:', micromark('`x + 1`'));
console.log('list:', JSON.stringify(micromark('- a\n- b\n')));
console.log('blockquote:', micromark('> quoted'));
console.log('hr:', JSON.stringify(micromark('---')));
console.log('table-ish pipes:', micromark('a | b'));
console.log('autolink:', micromark('<https://example.com>'));
