// kiln eval smoke test (authored for kiln, not from porffor).
import { parse } from 'css-what';
const sel = parse('div > .a + #b');
console.log(sel.length, sel[0].length, sel[0][0].type, sel[0][0].name);
const attr = parse('input[type="text"]');
console.log(attr[0][0].type, attr[0][1].name, attr[0][1].action);
console.log(JSON.stringify(parse('*').length));
