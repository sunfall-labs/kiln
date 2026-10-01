// kiln eval smoke test (authored for kiln, not from porffor).
import indentString from 'indent-string';

console.log(indentString('Unicorns\nRainbows', 4));
console.log(JSON.stringify(indentString('a\nb', 2, { indent: '>>' })));
console.log(JSON.stringify(indentString('x', 3, { includeEmptyLines: true })));
console.log(JSON.stringify(indentString('no-indent', 0)));
