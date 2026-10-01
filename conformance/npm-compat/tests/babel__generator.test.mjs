// kiln eval smoke test (authored for kiln, not from porffor).
import parser from '@babel/parser';
import generate from '@babel/generator';
console.log(generate(parser.parse('const x=1;')).code);
console.log(generate(parser.parse('function add(a,b){return a+b;}')).code.split('\n').join(' '));
console.log(generate(parser.parse('let o = {a: 1, ...rest};')).code);
