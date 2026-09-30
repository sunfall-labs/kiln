// kiln eval smoke test (authored for kiln, not from porffor).
const {parse} = require('@babel/parser');
const ast = parse('const x: number = 1;', {sourceType: 'module', plugins: ['typescript']});
console.log(ast.program.body[0].type);
const ast2 = parse('import x from "y";', {sourceType: 'module'});
console.log(ast2.program.body[0].type, ast2.program.body[0].source.value);
console.log(parse('a?.b ?? c', {sourceType: 'module'}).program.body[0].expression.type);
console.log(parse('let [a, ...rest] = arr;', {sourceType: 'module'}).program.body[0].declarations[0].id.type);
