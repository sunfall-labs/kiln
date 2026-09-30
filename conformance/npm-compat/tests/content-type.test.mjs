// kiln eval smoke test (authored for kiln, not from porffor).
import { parse, format } from 'content-type';
const p = parse('text/html; charset=utf-8');
console.log(p.type);
console.log(p.parameters.charset);
console.log(format({ type: 'text/plain', parameters: { charset: 'utf-8' } }));
const p2 = parse('application/json');
console.log(p2.type);
console.log(JSON.stringify(p2.parameters));
