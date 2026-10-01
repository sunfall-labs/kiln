// kiln eval smoke test (authored for kiln, not from porffor).
import { parse, walk, generate } from 'css-tree';
const ast = parse('.a { color: red; margin: 0 } #b { padding: 2px }');
const selectors = [];
walk(ast, node => { if (node.type === 'Selector') selectors.push(generate(node)); });
console.log(JSON.stringify(selectors));
console.log(generate(ast).includes('color:red'));
const decls = [];
walk(ast, node => { if (node.type === 'Declaration') decls.push(node.property); });
console.log(JSON.stringify(decls));
console.log(ast.type);
