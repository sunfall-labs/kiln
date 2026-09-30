// kiln eval smoke test (authored for kiln, not from porffor).
const ts = require('typescript');
const out = ts.transpileModule('const x: number = 1;\nconsole.log(x + 1);', {
  compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020},
});
console.log(out.outputText.trim().split('\n').join(' | '));
console.log(ts.version);
console.log(ts.isIdentifier(ts.factory.createIdentifier('foo')));
console.log(ts.ScriptTarget.ES2020);
