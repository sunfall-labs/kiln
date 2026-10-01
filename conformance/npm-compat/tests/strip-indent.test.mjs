// kiln eval smoke test (authored for kiln, not from porffor).
import stripIndent from 'strip-indent';
console.log(JSON.stringify(stripIndent('  foo\n    bar')));
console.log(JSON.stringify(stripIndent('\tfoo\n\t\tbar')));
console.log(JSON.stringify(stripIndent('no indent')));
console.log(JSON.stringify(stripIndent('  a\n  b\n  c')));
