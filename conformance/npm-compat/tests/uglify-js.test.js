// kiln eval smoke test (authored for kiln, not from porffor).
const UglifyJS = require('uglify-js');

const r1 = UglifyJS.minify('function add(a, b) { return a + b; } console.log(add(1, 2));');
console.log('basic:', r1.code);

const r2 = UglifyJS.minify('var longName = 1; console.log(longName + longName);', { mangle: { toplevel: true } });
console.log('mangle:', r2.code);

const r3 = UglifyJS.minify('if (false) { foo(); } bar();', { compress: {} });
console.log('dead code:', r3.code);

const bad = UglifyJS.minify('function ( {');
console.log('syntax error reported:', !!bad.error);

const ast = UglifyJS.parse('var x = 1 + 2;');
let count = 0;
ast.walk(new UglifyJS.TreeWalker(function () { count++; }));
console.log('ast nodes walked:', count > 3);

const out = UglifyJS.minify('var x=1;', { output: { beautify: true } });
console.log('beautify:', JSON.stringify(out.code));
