// kiln eval smoke test (authored for kiln, not from porffor).
import { find, tokenize } from 'linkifyjs';

console.log(JSON.stringify(find('Go to https://example.com or mail ada@example.org now')));
const toks = [...tokenize('Visit https://example.com!')];
console.log(JSON.stringify(toks.map((t) => t.t + ':' + t.toString())));
console.log(JSON.stringify(find('no links here')));
