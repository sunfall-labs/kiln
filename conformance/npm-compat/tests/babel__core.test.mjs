// kiln eval smoke test (authored for kiln, not from porffor).
import {transformSync, parseSync, version} from '@babel/core';
const out = transformSync('const f = (a, b) => a + b;', {configFile: false, babelrc: false});
console.log(out.code.split('\n').join(' '));
console.log(version);
console.log(parseSync('let x = 1;', {configFile: false}).program.body[0].type);
console.log(transformSync('let z = [1, 2, 3].map((n) => n * 2);', {configFile: false, babelrc: false}).code);
